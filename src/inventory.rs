use std::sync::{Arc, Mutex, MutexGuard, Weak};

use fxhash::FxHashMap;
use json::{object, JsonValue};
use uuid::Uuid;

use crate::{
    net::MouseButton,
    registry::Item,
    world::{Entity, EntityData, WorldBlock},
};

#[derive(Clone)]
pub struct ItemStack {
    pub item_type: Arc<Item>,
    item_count: u32,
}
impl ItemStack {
    pub fn new(item_type: Arc<Item>, item_count: u32) -> Self {
        ItemStack {
            item_type,
            item_count,
        }
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
        self.item_count = (self.item_count as i32 + count).max(0) as u32;
    }
    pub fn get_count(&self) -> u32 {
        self.item_count
    }
}
pub struct Inventory {
    items: Box<[Option<ItemStack>]>,
    viewers: FxHashMap<Uuid, Weak<Entity>>,
    client_id: String,
    slots: Vec<(f32, f32)>,
}
impl Inventory {
    pub fn new<F>(size: u32, ui_creator: F) -> Self
    where
        F: FnOnce() -> Vec<(f32, f32)>,
    {
        Inventory {
            items: vec![None; size as usize].into_boxed_slice(),
            viewers: FxHashMap::default(),
            client_id: Uuid::new_v4().to_string(),
            slots: ui_creator.call_once(()),
        }
    }
    pub fn get_size(&self) -> u32 {
        self.items.len() as u32
    }
    pub fn get_item(&self, index: u32) -> Result<&Option<ItemStack>, ()> {
        self.items.get(index as usize).ok_or(())
    }
    pub fn set_item(&mut self, index: u32, item: Option<ItemStack>) -> Result<(), ()> {
        if index >= self.items.len() as u32 {
            return Err(());
        }
        self.items[index as usize] = match item {
            Some(item) => {
                if item.item_count == 0 {
                    None
                } else {
                    Some(item)
                }
            }
            None => None,
        };
        self.sync_slot(index);
        Ok(())
    }
    pub fn modify_item<F>(&mut self, index: u32, function: F) -> Result<(), ()>
    where
        F: FnOnce(&mut Option<ItemStack>),
    {
        if index >= self.items.len() as u32 {
            return Err(());
        }
        function.call_once((&mut self.items[index as usize],));
        let set_as_empty = match &self.items[index as usize] {
            Some(item) => item.item_count == 0,
            None => false,
        };
        if set_as_empty {
            self.items[index as usize] = None;
        }
        self.sync_slot(index);
        Ok(())
    }
    fn sync_slot(&mut self, index: u32) {
        for viewer in self.viewers.values() {
            viewer.upgrade().unwrap()
            .try_send_message(&crate::net::NetworkMessageS2C::GuiData(
                object! {id:self.get_slot_id(index),type:"editElement",data_type:"item", item: Self::item_to_json(&self.items[index as usize])},
            ))
            .unwrap();
        }
    }
    pub fn add_viewer(&mut self, viewer: Arc<Entity>) {
        let id = viewer.get_id();
        if self.viewers.contains_key(id) {
            return;
        }
        for item in self.items.iter().enumerate() {
            let slot = self.slots.get(item.0).unwrap();
            let json = object! {
                id: self.get_slot_id(item.0 as u32),
                type: "setElement",
                element_type: "slot",
                x: slot.0,
                y: slot.1,
                item: Self::item_to_json(item.1)
            };
            viewer
                .try_send_message(&crate::net::NetworkMessageS2C::GuiData(json))
                .unwrap();
        }
        self.viewers.insert(id.clone(), Arc::downgrade(&viewer));
    }
    pub fn remove_viewer(&mut self, viewer: Arc<Entity>) {
        if let Some(viewer) = self.viewers.remove(viewer.get_id()) {
            if let Some(entity) = viewer.upgrade() {
                entity
                    .try_send_message(&crate::net::NetworkMessageS2C::GuiData(
                        object! {type:"removeContainer","container":self.client_id.clone()},
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
            .map(|item| object! {item:item.item_type.id, count:item.item_count})
    }
    pub fn set_cursor(entity_data: &mut EntityData) {
        let item = entity_data.get_inventory_hand();
        let player = entity_data.player.upgrade().unwrap();
        if item.is_some() {
            player.try_send_message(&&crate::net::NetworkMessageS2C::GuiData(object! {"type":"setElement",id:"cursor",element_type:"slot",background:false,item: Self::item_to_json(item)})).ok();
        } else {
            player.try_send_message(&&crate::net::NetworkMessageS2C::GuiData(object! {"type":"setElement",id:"cursor",element_type:"image",texture:"cursor",w:0.05,h:0.05})).ok();
        }
    }
    pub fn resolve_slot(&self, id: &str) -> Option<u32> {
        if id.starts_with(&self.client_id) {
            Some(id.to_string().replace(&self.client_id, "").parse().unwrap())
        } else {
            None
        }
    }
    pub fn on_click_slot(&mut self, player: &Entity, id: u32, button: MouseButton, shifting: bool) {
        let mut player_data = player.entity_data.lock().unwrap();
        if button == MouseButton::LEFT {
            let hand = player_data.get_inventory_hand().clone();
            player_data.set_inventory_hand(self.get_item(id).unwrap().clone());
            self.set_item(id, hand).unwrap();
        }
    }
    pub fn on_scroll_slot(&mut self, player: &Entity, id: u32, x: i32, y: i32, shifting: bool) {
        let mut player_data = player.entity_data.lock().unwrap();
        player_data.modify_inventory_hand(|first| {
            self.modify_item(id, |second| {
                let (first, second) = if y < 0 {
                    (first, second)
                } else {
                    (second, first)
                };

                if first.is_none() {
                    return;
                }
                let first_type = first.as_ref().unwrap().get_type();
                if second.is_none() {
                    *second = Some(ItemStack::new(first_type.clone(), 1));
                } else if Arc::ptr_eq(first_type, second.as_ref().unwrap().get_type()) {
                    second.as_mut().unwrap().add_count(1);
                }
                first.as_mut().unwrap().add_count(-1);
            })
            .unwrap();
        });
    }
}

#[derive(Clone)]
pub enum InventoryWrapper {
    Entity(Arc<Entity>),
    Block(Arc<WorldBlock>),
    Own(Arc<Mutex<Inventory>>),
}
impl InventoryWrapper {
    pub fn get_inventory(&self) -> Option<MutexGuard<Inventory>> {
        match self {
            Self::Entity(entity) => Some(entity.inventory.lock().unwrap()),
            Self::Block(block) => Some(block.inventory.lock().unwrap()),
            Self::Own(inventory) => Some(inventory.lock().unwrap()),
        }
    }
}
