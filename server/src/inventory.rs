use std::collections::HashMap;
use std::{
    ops::Range,
    sync::{Arc, Weak},
};

use bbscript::eval::{ExecutionEnvironment, ScriptError};
use bbscript::lex::FilePosition;
use bbscript::variant::{FromVariant, FunctionVariant, IntoVariant, Variant};
use block_byte_common::gui::{
    GUIComponent, GUIComponentEdit, GUIElement, GUIElementEdit, PositionAnchor,
};
use block_byte_common::messages::{ClientModelTarget, MouseButton, NetworkMessageS2C};
use block_byte_common::{Color, Position, Vec2};
use fxhash::FxHashMap;
use immutable_string::ImmutableString;
use json::{object, JsonValue};
use parking_lot::{Mutex, MutexGuard};
use rand::{thread_rng, Rng};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::mods::{ScriptCallback, ScriptingObject, UserDataWrapper};
use crate::world::{PlayerData, UserData};
use crate::worldgen::Spline;
use crate::{
    mods,
    registry::{InteractionResult, Item, ItemRegistry},
    util::Identifier,
    world::{Entity, WorldBlock},
    Server,
};

#[derive(Clone)]
pub struct ItemStack {
    pub item_type: Arc<Item>,
    item_count: u32,
}
impl ItemStack {
    pub fn new(item_type: &Arc<Item>, item_count: u32) -> Self {
        ItemStack {
            item_type: item_type.clone(),
            item_count: item_count.min(item_type.stack_size),
        }
    }
    pub fn from_json(json: &JsonValue, item_registry: &ItemRegistry) -> Result<Self, ()> {
        item_registry
            .item_by_identifier(&Identifier::parse(json["id"].as_str().unwrap()).unwrap())
            .map(|item| Self::new(item, json["count"].as_u32().unwrap_or(1)))
            .ok_or(())
    }
    pub fn copy(&self, new_count: u32) -> Self {
        ItemStack {
            item_type: self.item_type.clone(),
            item_count: new_count,
        }
    }
    pub fn get_type(&self) -> &Arc<Item> {
        &self.item_type
    }
    pub fn set_count(&mut self, count: u32) {
        self.item_count = count;
    }
    pub fn add_count(&mut self, count: i32) {
        self.item_count = (self.item_count as i32 + count)
            .max(0)
            .min(self.item_type.stack_size as i32) as u32;
    }
    pub fn get_count(&self) -> u32 {
        self.item_count
    }
}
impl ScriptingObject for ItemStack {
    fn engine_register(env: &mut ExecutionEnvironment, server: &Weak<Server>) {
        env.register_member("id", |item: &ItemStack| {
            Some(Variant::from_str(item.item_type.id.to_string().as_str()))
        });
        {
            let server = server.clone();
            env.register_function("ItemStack", move |item: &ImmutableString, count: &i64| {
                Ok(ItemStack::new(
                    server
                        .upgrade()
                        .unwrap()
                        .item_registry
                        .item_by_identifier(&Identifier::parse(item.as_ref()).unwrap())
                        .unwrap(),
                    *count as u32,
                ))
            });
        }
        env.register_member("count", |item: &ItemStack| Some(item.item_count as i64));
        env.register_member("stack_size", |item: &ItemStack| {
            Some(item.item_type.stack_size as i64)
        });
        env.register_method("with_count", |item: &ItemStack, new_count: &i64| {
            Ok(ItemStack::new(item.get_type(), *new_count as u32))
        });
    }
}
pub type InventorySetItemHandler = Box<dyn Fn(&Inventory, u32) + Send + Sync>;

pub struct Inventory {
    owner: WeakInventoryWrapper,
    items: Mutex<Box<[Option<ItemStack>]>>,
    viewers: Mutex<FxHashMap<Uuid, Arc<GuiInventoryViewer>>>,
    pub user_data: Mutex<UserData>,
    set_item_handler: Option<InventorySetItemHandler>,
    client_properties: Mutex<HashMap<ImmutableString, Variant>>,
}
impl Inventory {
    pub fn new_owned(size: u32, set_item_handler: Option<InventorySetItemHandler>) -> Arc<Self> {
        let inventory = Arc::new_cyclic(|this| Inventory {
            items: Mutex::new(vec![None; size as usize].into_boxed_slice()),
            viewers: Mutex::new(FxHashMap::default()),
            user_data: Mutex::new(UserData::new()),
            set_item_handler,
            owner: WeakInventoryWrapper::Own(this.clone()),
            client_properties: Mutex::new(HashMap::new()),
        });
        inventory
    }
    pub fn new<T>(owner: T, size: u32, set_item_handler: Option<InventorySetItemHandler>) -> Self
    where
        T: Into<WeakInventoryWrapper>,
    {
        Inventory {
            items: Mutex::new(vec![None; size as usize].into_boxed_slice()),
            viewers: Mutex::new(FxHashMap::default()),
            user_data: Mutex::new(UserData::new()),
            set_item_handler,
            owner: owner.into(),
            client_properties: Mutex::new(HashMap::new()),
        }
    }
    pub fn set_client_property(&self, id: &str, value: Variant, server: &Server) {
        let previous = self
            .client_properties
            .lock()
            .remove(id)
            .unwrap_or(Variant::NULL());
        for viewer in self.viewers.lock().iter() {
            viewer
                .1
                .layout
                .on_client_property
                .call_function(
                    &server.script_environment,
                    None,
                    vec![
                        ModGuiViewer {
                            viewer: viewer.1.viewer.clone(),
                            id: viewer.1.id.clone(),
                        }
                        .into_variant(),
                        Variant::from_str(id),
                        value.clone(),
                        previous.clone(),
                    ],
                )
                .unwrap();
        }
        self.client_properties.lock().insert(id.into(), value);
    }
    pub fn get_user_data(&self) -> MutexGuard<UserData> {
        self.user_data.lock()
    }
    pub fn export_content(&self) -> Box<[Option<ItemStack>]> {
        self.items.lock().clone()
    }
    pub fn load_content(&self, content: Box<[Option<ItemStack>]>) {
        let length = content.len();
        *self.items.lock() = content;
        for i in 0..length {
            self.sync_slot(i as u32, false);
        }
    }
    pub fn get_owner(&self) -> &WeakInventoryWrapper {
        &self.owner
    }
    pub fn ptr(&self) -> InventoryWrapper {
        self.owner.upgrade().unwrap()
    }
    pub fn get_size(&self) -> u32 {
        self.items.lock().len() as u32
    }
    fn sync_slot(&self, index: u32, only_count: bool) {
        let item = &self.items.lock()[index as usize];
        for viewer in self.viewers.lock().values() {
            if viewer.slot_range.contains(&index) {
                viewer
                    .viewer
                    .send_message(&NetworkMessageS2C::GuiEditElement(
                        format!(
                            "{}_{}",
                            viewer.id.to_string().as_str(),
                            (index - viewer.slot_range.start).to_string()
                        ),
                        GUIElementEdit {
                            component_type: GUIComponentEdit::SlotComponent {
                                item_id: Some(
                                    item.as_ref()
                                        .map(|item| (item.item_type.client_id, item.item_count)),
                                ),
                                size: None,
                                background: None,
                            },
                            ..Default::default()
                        },
                    ));
            }
        }
        if !only_count {
            match &self.owner.upgrade().unwrap() {
                InventoryWrapper::Entity(entity) => {
                    if let Some(mapping) = entity.entity_type.item_model_mapping.mapping.get(&index)
                    {
                        entity.get_location().chunk.announce_to_viewers(
                            &NetworkMessageS2C::ModelItem(
                                ClientModelTarget::Entity(entity.client_id),
                                *mapping,
                                item.as_ref().map(|item| item.item_type.client_id),
                            ),
                        );
                    }
                    if index == *entity.slot.lock() {
                        entity.sync_main_hand_viewmodel(item.as_ref());
                    }
                }
                InventoryWrapper::Block(block) => {
                    let chunk = block.chunk.upgrade().unwrap();
                    let block_type = chunk.world.server.block_registry.state_by_ref(block.state);
                    if let Some(mapping) = block_type.parent.item_model_mapping.mapping.get(&index)
                    {
                        chunk.announce_to_viewers(&NetworkMessageS2C::ModelItem(
                            ClientModelTarget::Block(block.position),
                            *mapping,
                            item.as_ref().map(|item| item.item_type.client_id),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    pub fn add_viewer(&self, viewer: GuiInventoryViewer) {
        let id = &viewer.id;
        if self.viewers.lock().contains_key(id) {
            return;
        }
        viewer
            .layout
            .send_to_player(&viewer.viewer, viewer.id.to_string().as_str());
        for slot in viewer.slot_range.clone() {
            let item = self
                .items
                .lock()
                .get(slot as usize)
                .unwrap()
                .as_ref()
                .map(|item| (item.item_type.client_id, item.item_count));
            viewer
                .viewer
                .send_message(&NetworkMessageS2C::GuiEditElement(
                    format!(
                        "{}_{}",
                        viewer.id.to_string().as_str(),
                        (slot - viewer.slot_range.start).to_string()
                    ),
                    GUIElementEdit {
                        component_type: GUIComponentEdit::SlotComponent {
                            item_id: Some(item),
                            size: None,
                            background: None,
                        },
                        ..Default::default()
                    },
                ));
        }
        for property in self.client_properties.lock().iter() {
            let _ = viewer.layout.on_client_property.call_function(
                &viewer.viewer.server.script_environment,
                None,
                vec![
                    ModGuiViewer {
                        viewer: viewer.viewer.clone(),
                        id: viewer.id.clone(),
                    }
                    .into_variant(),
                    Variant::from_str(property.0.to_string().as_str()),
                    property.1.clone(),
                    Variant::NULL(),
                ],
            );
        }
        self.viewers.lock().insert(id.clone(), Arc::new(viewer));
    }
    pub fn remove_viewer(&self, id: &Uuid) {
        if let Some(viewer) = self.viewers.lock().remove(id) {
            viewer
                .viewer
                .send_message(&NetworkMessageS2C::GuiRemoveElements(viewer.id.to_string()));
        }
    }
    pub fn get_slot_id_entity(&self, entity: &Entity, slot: u32) -> String {
        format!(
            "{}_{}",
            self.viewers
                .lock()
                .get(entity.get_id())
                .unwrap()
                .id
                .to_string(),
            slot
        )
    }
    fn item_to_json(item: &Option<ItemStack>) -> Option<JsonValue> {
        item.as_ref()
            .map(|item| object! {item:item.item_type.client_id, count:item.item_count})
    }
    pub fn set_cursor(player: &PlayerData, item: &Option<ItemStack>) {
        if item.is_some() {
            player.send_message(&NetworkMessageS2C::GuiSetElement(
                "item_cursor".to_string(),
                GUIElement {
                    component_type: GUIComponent::SlotComponent {
                        item_id: {
                            let item = item.as_ref().unwrap();
                            Some((item.item_type.client_id, item.item_count))
                        },
                        size: Vec2 { x: 100., y: 100. },
                        background: "".to_string(),
                    },
                    anchor: PositionAnchor::Cursor,
                    position: Position {
                        x: 0.,
                        y: 0.,
                        z: 10.,
                    },
                    base_color: Color::WHITE,
                },
            ));
        } else {
            player.send_message(&NetworkMessageS2C::GuiRemoveElements(
                "item_cursor".to_string(),
            ));
        }
    }
    pub fn resolve_id(&self, view_id: &Uuid, id: &str) -> Option<String> {
        let viewers = self.viewers.lock();
        let viewer = viewers.get(view_id).unwrap();
        if id.starts_with(&viewer.id.to_string()) {
            Some(
                id.to_string()
                    .replace(format!("{}_", &viewer.id.to_string()).as_str(), ""),
            )
        } else {
            None
        }
    }
    pub fn on_click(
        &self,
        viewer_id: Uuid,
        player: &PlayerData,
        id: &str,
        button: MouseButton,
        shifting: bool,
    ) {
        let slot = id.parse::<u32>().ok();
        let result = {
            let viewer = {
                let viewers = self.viewers.lock();
                viewers.get(&viewer_id).unwrap().clone()
            };
            InteractionResult::from_variant(
                &match slot {
                    Some(slot) => viewer.on_click.call_function(
                        &player.server.script_environment,
                        None,
                        vec![
                            player.ptr().into_variant(),
                            OwnedInventoryView::new(viewer.slot_range.clone(), self.ptr())
                                .into_variant(),
                            (slot as i64).into_variant(),
                            button.into_variant(),
                            shifting.into_variant(),
                        ],
                    ),
                    None => viewer.on_click.call_function(
                        &player.server.script_environment,
                        None,
                        vec![
                            player.ptr().into_variant(),
                            OwnedInventoryView::new(viewer.slot_range.clone(), self.ptr())
                                .into_variant(),
                            Variant::from_str(id.to_string().as_str()),
                            button.into_variant(),
                            shifting.into_variant(),
                        ],
                    ),
                }
                .unwrap(),
            )
            .cloned()
            .unwrap_or(InteractionResult::Ignored)
        };
        if let InteractionResult::Ignored = result {
            if button == MouseButton::Left {
                if let Some(slot_id) = slot {
                    let mut hand = player.hand_item.lock().clone();
                    let mut slot = self.get_full_view().get_item(slot_id).unwrap().clone();
                    match (hand.as_mut(), slot.as_mut()) {
                        (Some(hand), Some(slot)) => {
                            if Arc::ptr_eq(hand.get_type(), slot.get_type()) {
                                if hand.get_count() < hand.item_type.stack_size
                                    && slot.get_count() < slot.item_type.stack_size
                                {
                                    let transfer = (hand.get_type().stack_size - hand.get_count())
                                        .min(slot.get_count())
                                        as i32;
                                    hand.add_count(transfer);
                                    slot.add_count(-transfer);
                                }
                            }
                        }
                        _ => {}
                    }
                    player.set_inventory_hand(slot);
                    self.get_full_view().set_item(slot_id, hand).unwrap();
                }
            }
        }
    }
    pub fn on_scroll(
        &self,
        viewer_id: Uuid,
        player: &PlayerData,
        id: &str,
        x: i32,
        y: i32,
        shifting: bool,
    ) {
        let slot = id.parse::<u32>().ok();
        let result = {
            let viewer = {
                let viewers = self.viewers.lock();
                viewers.get(&viewer_id).unwrap().clone()
            };
            InteractionResult::from_variant(
                &match slot {
                    Some(slot) => viewer.on_scroll.call_function(
                        &player.server.script_environment,
                        None,
                        vec![
                            player.ptr().into_variant(),
                            OwnedInventoryView::new(viewer.slot_range.clone(), self.ptr())
                                .into_variant(),
                            (slot as i64).into_variant(),
                            (x as i64).into_variant(),
                            (y as i64).into_variant(),
                            shifting.into_variant(),
                        ],
                    ),
                    None => viewer.on_scroll.call_function(
                        &player.server.script_environment,
                        None,
                        vec![
                            player.ptr().into_variant(),
                            OwnedInventoryView::new(viewer.slot_range.clone(), self.ptr())
                                .into_variant(),
                            Variant::from_str(id.to_string().as_str()),
                            (x as i64).into_variant(),
                            (y as i64).into_variant(),
                            shifting.into_variant(),
                        ],
                    ),
                }
                .unwrap(),
            )
            .cloned()
            .unwrap_or(InteractionResult::Ignored)
        };
        if let InteractionResult::Ignored = result {
            if let Some(slot) = slot {
                player.modify_inventory_hand(|first| {
                    self.get_full_view()
                        .modify_item(slot, |second| {
                            let (first, second) = if y < 0 {
                                (first, second)
                            } else {
                                (second, first)
                            };

                            if let Some(first) = first {
                                match second {
                                    Some(second) => {
                                        if Arc::ptr_eq(first.get_type(), second.get_type())
                                            && second.get_count() < second.get_type().stack_size
                                        {
                                            second.add_count(1);
                                            first.add_count(-1);
                                        }
                                    }
                                    None => {
                                        *second = Some(ItemStack::new(first.get_type(), 1));
                                        first.add_count(-1);
                                    }
                                }
                            }
                        })
                        .unwrap();
                });
            }
        }
    }
    pub fn serialize(&self) -> InventorySaveData {
        InventorySaveData {
            items: self
                .items
                .lock()
                .iter()
                .map(|item| {
                    item.as_ref()
                        .map(|item| (item.item_type.id.to_string(), item.item_count))
                })
                .collect(),
        }
    }
    pub fn deserialize(
        &self,
        inventory_save_data: InventorySaveData,
        item_registry: &ItemRegistry,
    ) {
        let items: Vec<_> = inventory_save_data
            .items
            .iter()
            .map(|item| {
                item.as_ref().map(|item| {
                    ItemStack::new(
                        item_registry
                            .item_by_identifier(&Identifier::parse(item.0.as_str()).unwrap())
                            .unwrap(),
                        item.1,
                    )
                })
            })
            .collect();
        self.load_content(items.into_boxed_slice());
    }
    pub fn get_view(&self, slot_range: Range<u32>) -> InventoryView {
        InventoryView {
            slot_range,
            inventory: self,
        }
    }
    pub fn get_full_view(&self) -> InventoryView {
        self.get_view(0..self.get_size())
    }
}
#[derive(Serialize, Deserialize)]
pub struct InventorySaveData {
    items: Vec<Option<(String, u32)>>,
}
#[derive(Clone)]
pub struct OwnedInventoryView {
    slot_range: Range<u32>,
    inventory: InventoryWrapper,
}
impl OwnedInventoryView {
    pub fn new(slot_range: Range<u32>, inventory: InventoryWrapper) -> Self {
        OwnedInventoryView {
            slot_range,
            inventory,
        }
    }
    pub fn view(&self) -> InventoryView {
        self.inventory
            .get_inventory()
            .get_view(self.slot_range.clone())
    }
}
impl ScriptingObject for OwnedInventoryView {
    fn engine_register(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        env.register_custom_name::<OwnedInventoryView, _>("InventoryView");
        env.register_method("get_item", |view: &OwnedInventoryView, index: &i64| {
            Ok(Variant::from_option(
                view.view().get_item(*index as u32).unwrap(),
            ))
        });
        env.register_method(
            "set_item",
            |view: &OwnedInventoryView, index: &i64, item: &Variant| {
                view.view()
                    .set_item(
                        *index as u32,
                        Variant::into_option(item, &FilePosition::INVALID)?.cloned(),
                    )
                    .map_err(|_| {
                        ScriptError::runtime(
                            "inventory view access out of bounds",
                            FilePosition::INVALID,
                        )
                    })?;
                Ok(())
            },
        );
        env.register_method("add_item", |view: &OwnedInventoryView, item: &ItemStack| {
            Ok(Variant::from_option(view.view().add_item(item)))
        });
        env.register_method("get_inventory", |view: &OwnedInventoryView| {
            Ok(view.inventory.clone())
        });
    }
}
pub struct GuiInventoryData {
    pub slot_range: Range<u32>,
    pub layout: Arc<GUILayout>,
    pub on_click: ScriptCallback,
    pub on_scroll: ScriptCallback,
}
impl GuiInventoryData {
    pub fn into_viewer(self, viewer: Arc<PlayerData>) -> GuiInventoryViewer {
        GuiInventoryViewer {
            slot_range: self.slot_range,
            viewer,
            id: Uuid::new_v4(),
            layout: self.layout,
            on_click: self.on_click,
            on_scroll: self.on_scroll,
        }
    }
}
#[derive(Clone)]
pub struct ModGuiViewer {
    pub viewer: Arc<PlayerData>,
    pub id: Uuid,
}
impl ScriptingObject for ModGuiViewer {
    fn engine_register(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        env.register_custom_name::<ModGuiViewer, _>("GUIViewer");
        env.register_method(
            "set_text",
            |viewer: &ModGuiViewer, element_id: &ImmutableString, text: &ImmutableString| {
                viewer
                    .viewer
                    .send_message(&NetworkMessageS2C::GuiEditElement(
                        format!("{}_{}", viewer.id.to_string(), element_id),
                        GUIElementEdit {
                            component_type: GUIComponentEdit::TextComponent {
                                text: Some(text.to_string()),
                                font_size: None,
                            },
                            ..Default::default()
                        },
                    ));
                Ok(())
            },
        );
        env.register_method(
            "set_slice",
            |viewer: &ModGuiViewer,
             element_id: &ImmutableString,
             u1: &f64,
             v1: &f64,
             u2: &f64,
             v2: &f64| {
                viewer
                    .viewer
                    .send_message(&NetworkMessageS2C::GuiEditElement(
                        format!("{}_{}", viewer.id.to_string(), element_id),
                        GUIElementEdit {
                            component_type: GUIComponentEdit::ImageComponent {
                                slice: Some(Some((
                                    Vec2 {
                                        x: *u1 as f32,
                                        y: *v1 as f32,
                                    },
                                    Vec2 {
                                        x: *u2 as f32,
                                        y: *v2 as f32,
                                    },
                                ))),
                                size: None,
                                texture: None,
                            },
                            ..Default::default()
                        },
                    ));
                Ok(())
            },
        );
        env.register_method(
            "clear_slice",
            |viewer: &ModGuiViewer, element_id: &ImmutableString| {
                viewer
                    .viewer
                    .send_message(&NetworkMessageS2C::GuiEditElement(
                        format!("{}_{}", viewer.id.to_string(), element_id),
                        GUIElementEdit {
                            component_type: GUIComponentEdit::ImageComponent {
                                slice: Some(None),
                                size: None,
                                texture: None,
                            },
                            ..Default::default()
                        },
                    ));
                Ok(())
            },
        );
    }
}
pub struct GuiInventoryViewer {
    pub slot_range: Range<u32>,
    pub viewer: Arc<PlayerData>,
    pub id: Uuid,
    pub layout: Arc<GUILayout>,
    pub on_click: ScriptCallback,
    pub on_scroll: ScriptCallback,
}
impl GuiInventoryViewer {
    pub fn view<'a>(&self, inventory: &'a Inventory) -> InventoryView<'a> {
        inventory.get_view(self.slot_range.clone())
    }
}
pub struct InventoryView<'a> {
    slot_range: Range<u32>,
    inventory: &'a Inventory,
}
impl<'a> InventoryView<'a> {
    pub fn get_size(&self) -> u32 {
        self.slot_range.len() as u32
    }
    pub fn get_inventory(&self) -> &Inventory {
        self.inventory
    }
    pub fn map_slot(&self, index: u32) -> Result<u32, ()> {
        if index < self.get_size() {
            Ok(index + self.slot_range.start)
        } else {
            Err(())
        }
    }
    pub fn export_content(&self) -> Box<[Option<ItemStack>]> {
        let mut items = Vec::with_capacity(self.get_size() as usize);
        for i in 0..self.get_size() {
            items.push(self.get_item(i).unwrap());
        }
        items.into_boxed_slice()
    }
    pub fn get_item(&self, index: u32) -> Result<Option<ItemStack>, ()> {
        self.inventory
            .items
            .lock()
            .get(self.map_slot(index)? as usize)
            .cloned()
            .ok_or(())
    }
    pub fn set_item(&self, index: u32, item: Option<ItemStack>) -> Result<(), ()> {
        let index = self.map_slot(index)?;
        let only_count = {
            let mut items = self.inventory.items.lock();
            let old_item = items.get_mut(index as usize).unwrap();
            let only_count = match (old_item.as_ref(), item.as_ref()) {
                (Some(a), Some(b)) => Arc::ptr_eq(a.get_type(), b.get_type()),
                _ => false,
            };
            *old_item = match item {
                Some(item) => {
                    if item.item_count == 0 {
                        None
                    } else {
                        Some(item)
                    }
                }
                None => None,
            };
            only_count
        };
        self.inventory.sync_slot(index, only_count);

        if let Some(handler) = self.inventory.set_item_handler.as_ref() {
            handler.call((self.inventory, index));
        }
        Ok(())
    }
    pub fn modify_item<F>(&self, index: u32, function: F) -> Result<(), ()>
    where
        F: FnOnce(&mut Option<ItemStack>),
    {
        let index = self.map_slot(index)?;
        let only_count = {
            let mut items = self.inventory.items.lock();
            let item = items.get_mut(index as usize).unwrap();
            let old_item_type = item.as_ref().map(|item| item.item_type.clone());
            function.call_once((item,));
            let set_as_empty = match item {
                Some(item) => item.item_count == 0,
                None => false,
            };
            if set_as_empty {
                *item = None;
            }
            match (old_item_type, item) {
                (Some(a), Some(b)) => Arc::ptr_eq(&a, b.get_type()),
                _ => false,
            }
        };
        self.inventory.sync_slot(index, only_count);
        Ok(())
    }
    pub fn add_item(&self, item: &ItemStack) -> Option<ItemStack> {
        let mut rest = item.get_count();
        for slot in 0..self.get_size() {
            self.modify_item(slot as u32, |slot_item| {
                let set_rest = match slot_item {
                    Some(slot_item) => {
                        if Arc::ptr_eq(item.get_type(), slot_item.get_type()) {
                            let transfer =
                                (slot_item.item_type.stack_size - slot_item.get_count()).min(rest);
                            slot_item.add_count(transfer as i32);
                            rest -= transfer;
                        }
                        false
                    }
                    None => true,
                };
                if set_rest {
                    *slot_item = Some(item.copy(rest));
                    rest = 0;
                }
            })
            .unwrap();
            if rest == 0 {
                return None;
            }
        }
        Some(item.copy(rest))
    }
    pub fn remove_item(&self, item: &ItemStack) -> Option<ItemStack> {
        let mut rest = item.get_count();
        for slot in 0..self.get_size() {
            self.modify_item(slot as u32, |slot_item| {
                if let Some(slot_item) = slot_item {
                    if Arc::ptr_eq(item.get_type(), slot_item.get_type()) {
                        let transfer = slot_item.get_count().min(rest);
                        slot_item.add_count(-(transfer as i32));
                        rest -= transfer;
                    }
                }
            })
            .unwrap();
            if rest == 0 {
                return None;
            }
        }
        Some(item.copy(rest))
    }
}

#[derive(Clone)]
pub enum InventoryWrapper {
    Entity(Arc<Entity>),
    Block(Arc<WorldBlock>),
    Own(Arc<Inventory>),
}
impl InventoryWrapper {
    pub fn get_inventory(&self) -> &Inventory {
        match self {
            Self::Entity(entity) => &entity.inventory,
            Self::Block(block) => &block.inventory,
            Self::Own(inventory) => inventory,
        }
    }
    pub fn downgrade(&self) -> WeakInventoryWrapper {
        match self {
            Self::Entity(entity) => WeakInventoryWrapper::Entity(Arc::downgrade(entity)),
            Self::Block(block) => WeakInventoryWrapper::Block(Arc::downgrade(block)),
            Self::Own(own) => WeakInventoryWrapper::Own(Arc::downgrade(own)),
        }
    }
}
impl ScriptingObject for InventoryWrapper {
    fn engine_register(env: &mut ExecutionEnvironment, server: &Weak<Server>) {
        env.register_custom_name::<InventoryWrapper, _>("Inventory");
        env.register_function("create_inventory", |size: &i64| {
            //todo: verify size
            Ok(InventoryWrapper::Own(Inventory::new_owned(
                *size as u32,
                None,
            )))
        });
        env.register_method(
            "view",
            |inventory: &InventoryWrapper, range_start: &i64, range_end: &i64| {
                Ok(OwnedInventoryView::new(
                    Range::<u32> {
                        start: *range_start as u32,
                        end: *range_end as u32,
                    },
                    inventory.clone(),
                ))
            },
        );
        env.register_method("full_view", |inventory: &InventoryWrapper| {
            Ok(OwnedInventoryView::new(
                0..inventory.get_inventory().get_size(),
                inventory.clone(),
            ))
        });
        env.register_member("user_data", |inventory: &InventoryWrapper| {
            Some(UserDataWrapper::Inventory(inventory.clone()))
        });
        {
            let server = server.clone();
            env.register_method(
                "set_client_property",
                move |inventory: &InventoryWrapper, id: &ImmutableString, value: &Variant| {
                    inventory.get_inventory().set_client_property(
                        id.as_ref(),
                        value.clone(),
                        &server.upgrade().unwrap(),
                    );
                    Ok(())
                },
            );
        }
    }
}
#[derive(Clone)]
pub enum WeakInventoryWrapper {
    Entity(Weak<Entity>),
    Block(Weak<WorldBlock>),
    Own(Weak<Inventory>),
}
impl WeakInventoryWrapper {
    pub fn upgrade(&self) -> Option<InventoryWrapper> {
        match self {
            Self::Entity(entity) => entity
                .upgrade()
                .map(|entity| InventoryWrapper::Entity(entity)),
            Self::Block(block) => block.upgrade().map(|block| InventoryWrapper::Block(block)),
            Self::Own(inventory) => inventory
                .upgrade()
                .map(|inventory| InventoryWrapper::Own(inventory)),
        }
    }
}
pub struct Recipe {
    pub id: Identifier,
    recipe_type: Identifier,
    input_items: Vec<ItemStack>,
    output_items: Vec<ItemStack>,
}
impl Recipe {
    pub fn from_json(id: Identifier, json: JsonValue, item_registry: &ItemRegistry) -> Self {
        let mut input_items = Vec::new();
        let mut output_items = Vec::new();
        for item_input in json["item_inputs"].members() {
            input_items.push(ItemStack::from_json(item_input, item_registry).unwrap());
        }
        for item_output in json["item_outputs"].members() {
            output_items.push(ItemStack::from_json(item_output, item_registry).unwrap());
        }
        Recipe {
            id,
            recipe_type: Identifier::parse(json["type"].as_str().unwrap()).unwrap(),
            input_items,
            output_items,
        }
    }
    pub fn get_icon(&self) -> ItemStack {
        self.output_items.get(0).unwrap().clone()
    }
    pub fn get_type(&self) -> &Identifier {
        &self.recipe_type
    }
    pub fn has_ingredients(&self, inventory: &InventoryView) -> bool {
        let inventory_copy = Inventory::new_owned(inventory.get_size(), None);
        let inventory_copy_view = inventory_copy.get_full_view();
        inventory_copy.load_content(inventory.export_content());
        for input_item in &self.input_items {
            if let Some(_) = inventory_copy_view.remove_item(input_item) {
                return false;
            }
        }
        true
    }
    pub fn has_output_space(&self, inventory: &InventoryView) -> bool {
        let inventory_copy = Inventory::new_owned(inventory.get_size(), None);
        let inventory_copy_view = inventory_copy.get_full_view();
        inventory_copy.load_content(inventory.export_content());
        for input_item in &self.output_items {
            if let Some(_) = inventory_copy_view.add_item(input_item) {
                return false;
            }
        }
        true
    }
    pub fn consume_inputs(&self, inventory: &InventoryView) -> Result<(), ()> {
        if !self.has_ingredients(inventory) {
            return Err(());
        }
        for item in &self.input_items {
            inventory.remove_item(item);
        }
        Ok(())
    }
    pub fn add_outputs(&self, inventory: &InventoryView) -> Result<(), ()> {
        if !self.has_output_space(inventory) {
            return Err(());
        }
        for item in &self.output_items {
            inventory.add_item(item);
        }
        Ok(())
    }
}
impl ScriptingObject for Recipe {
    fn engine_register(env: &mut ExecutionEnvironment, server: &Weak<Server>) {
        env.register_custom_name::<Arc<Recipe>, _>("Recipe");
        {
            let server = server.clone();
            env.register_function("Recipe", move |id: &ImmutableString| {
                Ok(Variant::from_option(
                    server
                        .upgrade()
                        .unwrap()
                        .recipes
                        .by_id(&Identifier::parse(id.as_ref()).unwrap()),
                ))
            });
        }
        {
            let server = server.clone();
            env.register_function("recipes_by_type", move |id: &ImmutableString| {
                let server = server.upgrade().unwrap();
                Ok(Arc::new(Mutex::new(
                    server
                        .recipes
                        .by_type(&Identifier::parse(id.as_ref()).unwrap())
                        .iter()
                        .cloned()
                        .map(|recipe| recipe.into_variant())
                        .collect::<Vec<_>>(),
                )))
            });
        }
        env.register_method(
            "has_ingredients",
            |recipe: &Arc<Recipe>, inventory: &OwnedInventoryView| {
                Ok(recipe.has_ingredients(&inventory.view()))
            },
        );
        env.register_method(
            "has_output_space",
            |recipe: &Arc<Recipe>, inventory: &OwnedInventoryView| {
                Ok(recipe.has_output_space(&inventory.view()))
            },
        );
        env.register_method(
            "consume_inputs",
            |recipe: &Arc<Recipe>, inventory: &OwnedInventoryView| {
                Ok(match recipe.consume_inputs(&inventory.view()) {
                    Ok(_) => true,
                    Err(_) => false,
                })
            },
        );
        env.register_method(
            "add_outputs",
            |recipe: &Arc<Recipe>, inventory: &OwnedInventoryView| {
                Ok(match recipe.add_outputs(&inventory.view()) {
                    Ok(_) => true,
                    Err(_) => false,
                })
            },
        );
        env.register_member("id", |recipe: &Arc<Recipe>| {
            Some(ImmutableString::from(recipe.id.to_string()))
        });
    }
}

pub struct LootTable {
    tables: Vec<(Arc<Item>, Spline, Variant)>,
}
impl LootTable {
    pub fn from_json(
        json: JsonValue,
        item_registry: &ItemRegistry,
        environment: &ExecutionEnvironment,
    ) -> Self {
        let mut tables = Vec::new();
        for table in json["tables"].members() {
            let condition = &table["condition"];
            let conditions = if !condition.is_null() {
                mods::json_to_variant(condition.clone(), environment)
            } else {
                Variant::NULL()
            };
            tables.push((
                item_registry
                    .item_by_identifier(&Identifier::parse(table["id"].as_str().unwrap()).unwrap())
                    .unwrap()
                    .clone(),
                Spline::from_json(&table["count"]),
                conditions,
            ));
        }
        Self { tables }
    }
    pub fn generate_items(
        &self,
        data: Variant,
        environment: &ExecutionEnvironment,
    ) -> Vec<ItemStack> {
        let mut items = Vec::new();
        for table in &self.tables {
            if !table
                .2
                .call(vec![data.clone()], environment, &FilePosition::INVALID)
                .map(|variant| *bool::from_variant(&variant).unwrap_or(&false))
                .unwrap_or(false)
            {
                continue;
            }
            let count = table
                .1
                .sample(thread_rng().gen_range((0.)..(1.)))
                .unwrap()
                .round() as u32;
            if count > 0 {
                items.push(ItemStack::new(&table.0, count));
            }
        }
        items
    }
}
impl ScriptingObject for LootTable {
    fn engine_register(env: &mut ExecutionEnvironment, server: &Weak<Server>) {
        env.register_custom_name::<Arc<LootTable>, _>("LootTable");
        {
            let server = server.clone();
            env.register_function("LootTable", move |id: &ImmutableString| {
                Ok(Variant::from_option(
                    server
                        .upgrade()
                        .unwrap()
                        .loot_tables
                        .get(&Identifier::parse(id.as_ref()).unwrap())
                        .cloned(),
                ))
            });
        }
        {
            let server = server.clone();
            env.register_method("generate", move |this: &Arc<LootTable>, data: &Variant| {
                Ok(
                    this.generate_items(
                        data.clone(),
                        &server.upgrade().unwrap().script_environment,
                    ),
                )
            });
        }
    }
}
pub struct GUILayout {
    elements: HashMap<String, GUIElement>,
    on_client_property: ScriptCallback,
}
impl GUILayout {
    pub fn from_json(mut json: JsonValue, environment: &ExecutionEnvironment) -> GUILayout {
        let on_client_property = json.remove("on_client_property");
        let on_client_property = if on_client_property.is_null() {
            ScriptCallback::empty()
        } else {
            ScriptCallback::from_function_variant(
                FunctionVariant::from_variant(&mods::json_to_variant(
                    on_client_property,
                    environment,
                ))
                .unwrap(),
            )
        };
        let mut elements = HashMap::new();
        for (key, element) in json["elements"].entries() {
            elements.insert(
                key.to_string(),
                serde_json::from_str(element.to_string().as_str()).unwrap(),
            );
        }
        GUILayout {
            on_client_property,
            elements,
        }
    }
    pub fn send_to_player(&self, player: &PlayerData, container_id: &str) {
        for element in &self.elements {
            player.send_message(&NetworkMessageS2C::GuiSetElement(
                format!("{}_{}", container_id, element.0),
                element.1.clone(),
            ));
        }
    }
    /*pub fn create_9x(rows: u32) -> GUILayout {
        let mut elements = HashMap::new();
        for y in 0..rows {
            for x in 0..9 {
                elements.insert(
                    (x + (y * 9)).to_string(),
                    GUIElement {
                        position: Position {
                            x: ((x as f64 - 4.) * 130.),
                            y: (y as f64 - (rows as f64 / 2.)) * 130.,
                            z: 0.,
                        },
                        anchor: PositionAnchor::Center,
                        base_color: Color::WHITE,
                        component_type: GUIComponent::SlotComponent {
                            item_id: None,
                            background: "bb:slot".to_string(),
                            size: Vec2 { x: 100., y: 100. },
                        },
                    },
                );
            }
        }
        GUILayout {
            elements,
            on_client_property: ScriptCallback::empty(),
        }
    }
    pub fn export_to_json(&self) -> String {
        serde_json::to_string(self).unwrap()
    }*/
}
