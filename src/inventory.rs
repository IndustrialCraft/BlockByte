use std::{
    collections::HashMap,
    hash::Hash,
    sync::{Arc, Weak},
};

use json::{object, JsonValue};
use uuid::Uuid;

use crate::{registry::Item, world::Entity};

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
    pub fn get_type_type(&self) -> &Arc<Item> {
        &self.item_type
    }
    pub fn set_count(&mut self, count: u32) {
        self.item_count = count;
    }
    pub fn add_count(&mut self, count: i32) {
        self.item_count = (self.item_count as i32 + count).max(0) as u32;
    }
}
pub struct Inventory {
    items: Box<[Option<ItemStack>]>,
    viewers: HashMap<Uuid, InventoryViewer>,
}
impl Inventory {
    pub fn new(size: u32) -> Self {
        Inventory {
            items: vec![None; size as usize].into_boxed_slice(),
            viewers: HashMap::new(),
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
            viewer.on_slot_update(index, &self.items[index as usize]);
        }
    }
    pub fn add_viewer(&mut self, viewer: InventoryViewer) {
        let entity = viewer.entity.upgrade().unwrap();
        let id = entity.get_id();
        if self.viewers.contains_key(id) {
            return;
        }
        viewer.on_open(&self.items);
        self.viewers.insert(id.clone(), viewer);
    }
    pub fn remove_viewer(&mut self, viewer: Arc<Entity>) {
        if let Some(viewer) = self.viewers.remove(viewer.get_id()) {
            viewer.on_close();
        }
    }
    pub fn get_viewer(&self, viewer: &Arc<Entity>) -> Option<&InventoryViewer> {
        self.viewers.get(viewer.get_id())
    }
}
pub struct InventoryViewer {
    entity: Weak<Entity>,
    client_id: String,
    slots: Vec<(f32, f32)>,
}
impl InventoryViewer {
    pub fn new(entity: Weak<Entity>, slots: Vec<(f32, f32)>) -> Self {
        InventoryViewer {
            entity,
            client_id: Uuid::new_v4().to_string(),
            slots,
        }
    }
    pub fn on_open(&self, items: &Box<[Option<ItemStack>]>) {
        for item in items.iter().enumerate() {
            let slot = self.slots.get(item.0).unwrap();
            let json = object! {
                id: self.get_slot_id(item.0 as u32),
                type: "setElement",
                element_type: "slot",
                x: slot.0,
                y: slot.1,
                item: Self::item_to_json(item.1)
            };
            self.entity
                .upgrade()
                .unwrap()
                .try_send_message(&crate::net::NetworkMessageS2C::GuiData(json))
                .unwrap();
        }
    }
    pub fn on_close(&self) {
        if let Some(entity) = self.entity.upgrade() {
            entity
                .try_send_message(&crate::net::NetworkMessageS2C::GuiData(
                    object! {type:"removeContainer","container":self.client_id.clone()},
                ))
                .unwrap();
        }
    }
    pub fn on_slot_update(&self, slot: u32, item: &Option<ItemStack>) {
        self.entity.upgrade().unwrap()
            .try_send_message(&crate::net::NetworkMessageS2C::GuiData(
                object! {id:self.get_slot_id(slot),type:"editElement",data_type:"item", item: Self::item_to_json(item)},
            ))
            .unwrap();
    }
    pub fn get_slot_id(&self, slot: u32) -> String {
        self.client_id.clone() + slot.to_string().as_str()
    }
    fn item_to_json(item: &Option<ItemStack>) -> Option<JsonValue> {
        item.as_ref()
            .map(|item| object! {item:item.item_type.id, count:item.item_count})
    }
}
impl Drop for InventoryViewer {
    fn drop(&mut self) {
        self.on_close()
    }
}
impl Hash for InventoryViewer {
    fn hash<H: ~const std::hash::Hasher>(&self, state: &mut H) {
        self.entity.upgrade().unwrap().get_id().hash(state)
    }
}
impl PartialEq for InventoryViewer {
    fn eq(&self, other: &Self) -> bool {
        self.entity.upgrade().unwrap().get_id() == other.entity.upgrade().unwrap().get_id()
    }
}
impl Eq for InventoryViewer {}
