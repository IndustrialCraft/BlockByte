use std::{
    ops::Range,
    sync::{Arc, Weak},
};

use block_byte_common::gui::{
    GUIComponent, GUIComponentEdit, GUIElement, GUIElementEdit, PositionAnchor,
};
use block_byte_common::messages::{MouseButton, NetworkMessageS2C};
use block_byte_common::{Color, Position, Vec2};
use fxhash::FxHashMap;
use json::{object, JsonValue};
use parking_lot::{Mutex, MutexGuard};
use rand::{thread_rng, Rng};
use serde::{Deserialize, Serialize};
use splines::Spline;
use uuid::Uuid;

use crate::world::UserData;
use crate::{
    registry::{InteractionResult, Item, ItemRegistry},
    util::Identifier,
    world::{Entity, EntityData, WorldBlock},
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
            item_count,
        }
    }
    pub fn from_json(json: &JsonValue, item_registry: &ItemRegistry) -> Result<Self, ()> {
        //todo: error instead of crashing
        Ok(Self::new(
            item_registry
                .item_by_identifier(&Identifier::parse(json["id"].as_str().unwrap()).unwrap())
                .unwrap(),
            json["count"].as_u32().unwrap(),
        ))
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
pub type InventoryClickHandler =
    Box<dyn Fn(&Inventory, &Entity, u32, MouseButton, bool) -> InteractionResult + Send + Sync>;
pub type InventoryScrollHandler =
    Box<dyn Fn(&Inventory, &Entity, u32, i32, i32, bool) -> InteractionResult + Send + Sync>;
pub type InventorySetItemHandler = Box<dyn Fn(&Inventory, u32) + Send + Sync>;

pub struct Inventory {
    owner: WeakInventoryWrapper,
    items: Mutex<Box<[Option<ItemStack>]>>,
    viewers: Mutex<FxHashMap<Uuid, Weak<Entity>>>,
    client_id: String,
    slots: Vec<(PositionAnchor, f32, f32)>,
    pub user_data: Mutex<UserData>,
    click_handler: Option<InventoryClickHandler>,
    scroll_handler: Option<InventoryScrollHandler>,
    set_item_handler: Option<InventorySetItemHandler>,
}
impl Inventory {
    pub fn new_owned<F>(
        size: u32,
        ui_creator: F,
        click_handler: Option<InventoryClickHandler>,
        scroll_handler: Option<InventoryScrollHandler>,
        set_item_handler: Option<InventorySetItemHandler>,
    ) -> Arc<Self>
    where
        F: FnOnce() -> Vec<(PositionAnchor, f32, f32)>,
    {
        let inventory = Arc::new_cyclic(|this| Inventory {
            items: Mutex::new(vec![None; size as usize].into_boxed_slice()),
            viewers: Mutex::new(FxHashMap::default()),
            client_id: Uuid::new_v4().to_string(),
            slots: ui_creator.call_once(()),
            user_data: Mutex::new(UserData::new()),
            click_handler,
            scroll_handler,
            set_item_handler,
            owner: WeakInventoryWrapper::Own(this.clone()),
        });
        inventory
    }
    pub fn new<F, T>(
        owner: T,
        size: u32,
        ui_creator: F,
        click_handler: Option<InventoryClickHandler>,
        scroll_handler: Option<InventoryScrollHandler>,
        set_item_handler: Option<InventorySetItemHandler>,
    ) -> Self
    where
        F: FnOnce() -> Vec<(PositionAnchor, f32, f32)>,
        T: Into<WeakInventoryWrapper>,
    {
        Inventory {
            items: Mutex::new(vec![None; size as usize].into_boxed_slice()),
            viewers: Mutex::new(FxHashMap::default()),
            client_id: Uuid::new_v4().to_string(),
            slots: ui_creator.call_once(()),
            user_data: Mutex::new(UserData::new()),
            click_handler,
            scroll_handler,
            set_item_handler,
            owner: owner.into(),
        }
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
            self.sync_slot(i as u32);
        }
    }
    pub fn get_owner(&self) -> &WeakInventoryWrapper {
        &self.owner
    }
    pub fn get_size(&self) -> u32 {
        self.items.lock().len() as u32
    }
    fn sync_slot(&self, index: u32) {
        let item = &self.items.lock()[index as usize];
        for viewer in self.viewers.lock().values() {
            viewer
                .upgrade()
                .unwrap()
                .try_send_message(&NetworkMessageS2C::GuiEditElement(
                    self.get_slot_id(index),
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
                ))
                .unwrap();
        }
        match &self.owner.upgrade().unwrap() {
            InventoryWrapper::Entity(entity) => {
                if let Some(mapping) = entity.entity_type.item_model_mapping.mapping.get(&index) {
                    entity.get_location().chunk.announce_to_viewers(
                        &NetworkMessageS2C::EntityItem(
                            entity.client_id,
                            *mapping,
                            item.as_ref().map(|item| item.item_type.client_id),
                        ),
                    );
                }
            }
            //todo: block
            _ => {}
        }
    }
    pub fn add_viewer(&self, viewer: Arc<Entity>) {
        let id = viewer.get_id();
        if self.viewers.lock().contains_key(id) {
            return;
        }
        for item in self.items.lock().iter().enumerate() {
            let slot = self.slots.get(item.0).unwrap();
            viewer
                .try_send_message(&NetworkMessageS2C::GuiSetElement(
                    self.get_slot_id(item.0 as u32),
                    GUIElement {
                        component_type: GUIComponent::SlotComponent {
                            background: "bb:slot".to_string(),
                            size: Vec2 { x: 100., y: 100. },
                            item_id: item
                                .1
                                .as_ref()
                                .map(|item| (item.item_type.client_id, item.item_count)),
                        },
                        position: Position {
                            x: slot.1 as f64,
                            y: slot.2 as f64,
                            z: 0.,
                        },
                        anchor: slot.0,
                        base_color: Color::WHITE,
                    },
                ))
                .unwrap();
        }
        self.viewers
            .lock()
            .insert(id.clone(), Arc::downgrade(&viewer));
    }
    pub fn remove_viewer(&self, viewer: Arc<Entity>) {
        if let Some(viewer) = self.viewers.lock().remove(viewer.get_id()) {
            if let Some(entity) = viewer.upgrade() {
                entity
                    .try_send_message(&NetworkMessageS2C::GuiRemoveElements(
                        self.client_id.clone(),
                    ))
                    .unwrap();
            }
        }
    }
    pub fn get_slot_id(&self, slot: u32) -> String {
        self.client_id.clone() + slot.to_string().as_str()
    }
    fn item_to_json(item: &Option<ItemStack>) -> Option<JsonValue> {
        item.as_ref()
            .map(|item| object! {item:item.item_type.client_id, count:item.item_count})
    }
    pub fn set_cursor(entity_data: &mut EntityData) {
        let item = entity_data.get_inventory_hand();
        let player = entity_data.player.upgrade().unwrap();
        if item.is_some() {
            player
                .try_send_message(&NetworkMessageS2C::GuiSetElement(
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
                ))
                .ok();
            //player.try_send_message(&&NetworkMessageS2C::GuiData(object! {"type":"setElement",id:"cursor",element_type:"slot",background:false,item: Self::item_to_json(item)}.to_string())).ok();
        } else {
            player
                .try_send_message(&NetworkMessageS2C::GuiRemoveElements(
                    "item_cursor".to_string(),
                ))
                .ok();
            //player.try_send_message(&&NetworkMessageS2C::GuiData(object! {"type":"setElement",id:"cursor",element_type:"image",texture:"cursor",w:0.05,h:0.05}.to_string())).ok();
        }
    }
    pub fn resolve_slot(&self, id: &str) -> Option<u32> {
        if id.starts_with(&self.client_id) {
            Some(id.to_string().replace(&self.client_id, "").parse().unwrap())
        } else {
            None
        }
    }
    pub fn on_click_slot(&self, player: &Entity, id: u32, button: MouseButton, shifting: bool) {
        let result = self
            .click_handler
            .as_ref()
            .map(|handler| handler.call((self, player, id, button, shifting)))
            .unwrap_or(InteractionResult::Ignored);
        if let InteractionResult::Ignored = result {
            let mut player_data = player.entity_data.lock();
            if button == MouseButton::Left {
                let mut hand = player_data.get_inventory_hand().clone();
                let mut slot = self.get_full_view().get_item(id).unwrap().clone();
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
                player_data.set_inventory_hand(slot);
                self.get_full_view().set_item(id, hand).unwrap();
            }
        }
    }
    pub fn on_scroll_slot(&self, player: &Entity, id: u32, x: i32, y: i32, shifting: bool) {
        let result = self
            .scroll_handler
            .as_ref()
            .map(|handler| handler.call((self, player, id, x, y, shifting)))
            .unwrap_or(InteractionResult::Ignored);
        if let InteractionResult::Ignored = result {
            let mut player_data = player.entity_data.lock();
            player_data.modify_inventory_hand(|first| {
                self.get_full_view()
                    .modify_item(id, |second| {
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
        if self.slot_range.contains(&index) {
            Ok(index - self.slot_range.start)
        } else {
            Err(())
        }
    }
    pub fn get_item(&self, index: u32) -> Result<Option<ItemStack>, ()> {
        //todo: prevent copying
        self.inventory
            .items
            .lock()
            .get(self.map_slot(index)? as usize)
            .cloned()
            .ok_or(())
    }
    pub fn set_item(&self, index: u32, item: Option<ItemStack>) -> Result<(), ()> {
        let index = self.map_slot(index)?;
        self.inventory.items.lock()[index as usize] = match item {
            Some(item) => {
                if item.item_count == 0 {
                    None
                } else {
                    Some(item)
                }
            }
            None => None,
        };
        self.inventory.sync_slot(index);
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

        function.call_once((&mut self.inventory.items.lock()[index as usize],));
        let set_as_empty = match &self.inventory.items.lock()[index as usize] {
            Some(item) => item.item_count == 0,
            None => false,
        };
        if set_as_empty {
            self.inventory.items.lock()[index as usize] = None;
        }
        self.inventory.sync_slot(index);
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
    pub fn can_craft(&self, inventory: &Inventory) -> bool {
        let content = inventory.export_content();
        let inventory_copy = inventory.get_full_view();
        for input_item in &self.input_items {
            if let Some(_) = inventory_copy.remove_item(input_item) {
                inventory.load_content(content);
                return false;
            }
        }
        inventory.load_content(content);
        true
    }
    pub fn consume_inputs(&self, inventory: &Inventory) -> Result<(), ()> {
        if !self.can_craft(inventory) {
            return Err(());
        }
        let inventory = inventory.get_full_view();
        for item in &self.input_items {
            inventory.remove_item(item);
        }
        Ok(())
    }
    pub fn add_outputs(&self, inventory: &Inventory) {
        let inventory = inventory.get_full_view();
        for item in &self.output_items {
            inventory.add_item(item);
        }
    }
}

pub struct LootTable {
    tables: Vec<(Arc<Item>, Spline<f64, f64>)>,
}
impl LootTable {
    pub fn from_json(json: JsonValue, item_registry: &ItemRegistry) -> Self {
        let mut tables = Vec::new();
        for table in json["tables"].members() {
            tables.push((
                item_registry
                    .item_by_identifier(&Identifier::parse(table["id"].as_str().unwrap()).unwrap())
                    .unwrap()
                    .clone(),
                crate::mods::spline_from_json(&table["count"]),
            ));
        }
        Self { tables }
    }
    pub fn generate_items<T>(&self, consumer: T)
    where
        T: Fn(ItemStack),
    {
        for table in &self.tables {
            let count = table
                .1
                .clamped_sample(thread_rng().gen_range((0.)..(1.)))
                .unwrap()
                .round() as u32;
            if count > 0 {
                consumer.call((ItemStack::new(&table.0, count),));
            }
        }
    }
}
