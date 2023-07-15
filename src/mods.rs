use std::{
    collections::HashMap,
    hash::BuildHasherDefault,
    path::{Path, PathBuf},
    rc,
    str::FromStr,
    sync::{Arc, Mutex},
};

use anyhow::{bail, Context, Result};

use rhai::{Dynamic, Engine, FnPtr, Func, ImmutableString, AST};
use twox_hash::XxHash64;
use walkdir::WalkDir;

use crate::{
    registry::{
        Block, BlockRegistry, BlockState, ClientBlockCubeRenderData, ClientBlockDynamicData,
        ClientBlockRenderData, ClientBlockRenderDataType, ClientEntityData, ClientItemModel,
        ClientItemRenderData, EntityRegistry, Item, ItemRegistry,
    },
    util::Identifier,
};

struct Mod {
    path: PathBuf,
    namespace: String,
}
impl Mod {
    pub fn new(path: &Path) -> Result<Self> {
        let mut path_buf = path.to_path_buf();
        path_buf.push("descriptor.json");
        let descriptor = json::parse(
            std::fs::read_to_string(&path_buf)
                .with_context(|| {
                    format!(
                        "descriptor for mod {} wasn't found",
                        path.file_name().unwrap().to_str().unwrap()
                    )
                })?
                .as_str(),
        )
        .with_context(|| {
            format!(
                "descriptor for mod {} is incorrect",
                path.file_name().unwrap().to_str().unwrap()
            )
        })?;
        path_buf.pop();
        let mod_identifier = descriptor["id"].as_str().unwrap().to_string();
        Ok(Mod {
            path: path.to_path_buf(),
            namespace: mod_identifier,
        })
    }
    pub fn load_scripts(&self, engine: &Engine) {
        for script in WalkDir::new({
            let mut scripts_path = self.path.clone();
            scripts_path.push("scripts");
            scripts_path
        })
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|entry| entry.metadata().unwrap().is_file())
        {
            if let Err(error) = engine.eval_file::<()>(script.into_path()) {
                println!("script error: {}", error.to_string());
            }
            //todo
        }
    }
    pub fn call_event<T>(&self, event: &str, param: T) {}
    pub fn read_resource(&self, id: Arc<Identifier>) -> Result<Vec<u8>> {
        if id.get_namespace() == &self.namespace {
            let mut full_path = self.path.clone();
            for path_part in id.get_key().split("/") {
                full_path.push(path_part);
            }
            std::fs::read(full_path).with_context(|| format!("resource {} not found", id))
        } else {
            bail!(
                "identifier {} doesn't have same namespace as mod {} it was requested from",
                id,
                self.namespace
            );
        }
    }
}

pub struct ModManager {
    mods: HashMap<String, Mod>,
}
impl ModManager {
    pub fn load_mods(
        path: &Path,
    ) -> (
        Self,
        Vec<BlockBuilder>,
        Vec<ItemBuilder>,
        Vec<EntityBuilder>,
        ClientContentData,
    ) {
        let mut mods = HashMap::new();
        for mod_path in std::fs::read_dir(path).unwrap() {
            let mod_path = mod_path.unwrap();
            let path = mod_path.path();
            let name = mod_path.file_name().to_str().unwrap().to_string();
            if let Ok(loaded_mod) = Mod::new(path.as_path()) {
                if mods.contains_key(&loaded_mod.namespace) {
                    panic!("mod {} loaded twice", loaded_mod.namespace);
                }
                mods.insert(loaded_mod.namespace.clone(), loaded_mod);
            } else {
                println!("loading mod '{}' failed", name);
            }
        }
        let mut loading_engine = Engine::new();
        let current_mod_path = Arc::new(Mutex::new(PathBuf::new()));
        let content = Arc::new(Mutex::new(ClientContentData::new()));
        let blocks = Arc::new(Mutex::new(Vec::new()));
        let items = Arc::new(Mutex::new(Vec::new()));
        let entities = Arc::new(Mutex::new(Vec::new()));
        let registered_blocks = blocks.clone();
        let registered_items = items.clone();
        let registered_entities = entities.clone();
        loading_engine
            .register_type_with_name::<BlockBuilder>("BlockBuilder")
            .register_fn("create_block", BlockBuilder::new)
            .register_fn("client_type_air", BlockBuilder::client_type_air)
            .register_fn("client_type_cube", BlockBuilder::client_type_cube)
            .register_fn("client_fluid", BlockBuilder::client_fluid)
            .register_fn("client_transparent", BlockBuilder::client_transparent)
            .register_fn("client_render_data", BlockBuilder::client_render_data)
            .register_fn("client_dynamic", BlockBuilder::client_dynamic)
            .register_fn(
                "client_dynamic_add_animation",
                BlockBuilder::client_dynamic_add_animation,
            )
            .register_fn(
                "client_dynamic_add_item",
                BlockBuilder::client_dynamic_add_item,
            )
            .register_fn("register", move |this: &mut Arc<Mutex<BlockBuilder>>| {
                registered_blocks.lock().unwrap().push(this.clone())
            });
        loading_engine
            .register_type_with_name::<ItemBuilder>("ItemBuilder")
            .register_fn("create_item", ItemBuilder::new)
            .register_fn("client_name", ItemBuilder::client_name)
            .register_fn("client_model_texture", ItemBuilder::client_model_texture)
            .register_fn("client_model_block", ItemBuilder::client_model_block)
            .register_fn("place", ItemBuilder::place)
            .register_fn("register", move |this: &mut Arc<Mutex<ItemBuilder>>| {
                registered_items.lock().unwrap().push(this.clone())
            });
        loading_engine
            .register_type_with_name::<EntityBuilder>("EntityBuilder")
            .register_fn("create_entity", EntityBuilder::new)
            .register_fn("client_model", EntityBuilder::client_model)
            .register_fn("client_hitbox", EntityBuilder::client_hitbox)
            .register_fn("client_add_animation", EntityBuilder::client_add_animation)
            .register_fn("client_add_item", EntityBuilder::client_add_item)
            .register_fn("tick", EntityBuilder::tick)
            .register_fn("register", move |this: &mut Arc<Mutex<EntityBuilder>>| {
                registered_entities.lock().unwrap().push(this.clone())
            });

        let mut content_register = |name: &str, content_type: ContentType| {
            let register_current_mod_path = current_mod_path.clone();
            let register_content = content.clone();
            loading_engine.register_fn(name, move |id: &str, path: &str| {
                let start_path = { register_current_mod_path.lock().unwrap().clone() };
                let mut full_path = start_path.clone();
                full_path.push(path);
                if !full_path.starts_with(start_path) {
                    panic!("path travelsal attack");
                }
                register_content
                    .lock()
                    .unwrap()
                    .by_type(content_type)
                    .insert(
                        Identifier::parse(id).unwrap(),
                        std::fs::read(full_path).unwrap(),
                    );
            });
        };
        content_register.call_mut(("register_image", ContentType::Image));
        content_register.call_mut(("register_sound", ContentType::Sound));
        content_register.call_mut(("register_model", ContentType::Model));
        for loaded_mod in &mods {
            {
                let mut path = current_mod_path.lock().unwrap();
                path.clear();
                path.push(loaded_mod.1.path.clone());
            }
            loaded_mod.1.load_scripts(&loading_engine);
        }
        let blocks = blocks
            .lock()
            .unwrap()
            .iter()
            .map(|block| block.lock().unwrap().clone())
            .collect();
        let items = items
            .lock()
            .unwrap()
            .iter()
            .map(|item| item.lock().unwrap().clone())
            .collect();
        let entities = entities
            .lock()
            .unwrap()
            .iter()
            .map(|entity| entity.lock().unwrap().clone())
            .collect();

        //println!("{blocks:#?}\n{items:#?}\n{entities:#?}");
        let content = content.lock().unwrap().clone();
        (ModManager { mods }, blocks, items, entities, content)
    }
    /*pub fn call_event<T>(&self, event: &str, param: T) {
        //todo
        for loaded_mod in &self.mods {
            loaded_mod.1.call_event(event, param.clone());
        }
    }*/
}
#[derive(Clone, Debug)]
pub struct BlockBuilder {
    pub id: Identifier,
    pub client: ClientBlockRenderData,
}
impl BlockBuilder {
    pub fn new(id: &str) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(BlockBuilder {
            id: Identifier::parse(id).unwrap(),
            client: ClientBlockRenderData {
                block_type: ClientBlockRenderDataType::Air,
                dynamic: None,
                fluid: false,
                render_data: 0,
                transparent: false,
            },
        }))
    }
    pub fn client_type_air(this: &mut Arc<Mutex<Self>>) -> Arc<Mutex<Self>> {
        this.lock().unwrap().client.block_type = ClientBlockRenderDataType::Air;
        this.clone()
    }
    pub fn client_type_cube(
        this: &mut Arc<Mutex<Self>>,
        front: &str,
        back: &str,
        right: &str,
        left: &str,
        up: &str,
        down: &str,
    ) -> Arc<Mutex<Self>> {
        this.lock().unwrap().client.block_type =
            ClientBlockRenderDataType::Cube(ClientBlockCubeRenderData {
                front: front.to_string(),
                back: back.to_string(),
                right: right.to_string(),
                left: left.to_string(),
                up: up.to_string(),
                down: down.to_string(),
            });
        this.clone()
    }
    pub fn client_fluid(this: &mut Arc<Mutex<Self>>, fluid: bool) -> Arc<Mutex<Self>> {
        this.lock().unwrap().client.fluid = fluid;
        this.clone()
    }
    pub fn client_transparent(this: &mut Arc<Mutex<Self>>, transparent: bool) -> Arc<Mutex<Self>> {
        this.lock().unwrap().client.transparent = transparent;
        this.clone()
    }
    pub fn client_render_data(this: &mut Arc<Mutex<Self>>, render_data: i64) -> Arc<Mutex<Self>> {
        this.lock().unwrap().client.render_data = render_data as u8;
        this.clone()
    }
    pub fn client_dynamic(
        this: &mut Arc<Mutex<Self>>,
        model: &str,
        texture: &str,
    ) -> Arc<Mutex<Self>> {
        this.lock().unwrap().client.dynamic = Some(ClientBlockDynamicData {
            model: model.to_string(),
            texture: texture.to_string(),
            animations: Vec::new(),
            items: Vec::new(),
        });
        this.clone()
    }
    pub fn client_dynamic_add_animation(
        this: &mut Arc<Mutex<Self>>,
        animation: &str,
    ) -> Arc<Mutex<Self>> {
        //todo: result
        if let Some(dynamic) = &mut this.lock().unwrap().client.dynamic {
            dynamic.animations.push(animation.to_string());
        }
        this.clone()
    }
    pub fn client_dynamic_add_item(this: &mut Arc<Mutex<Self>>, item: &str) -> Arc<Mutex<Self>> {
        //todo: result
        if let Some(dynamic) = &mut this.lock().unwrap().client.dynamic {
            dynamic.items.push(item.to_string());
        }
        this.clone()
    }
}
#[derive(Clone)]
pub struct ItemBuilder {
    pub id: Identifier,
    pub client: ClientItemRenderData,
    pub place: Option<Identifier>,
}
impl ItemBuilder {
    pub fn new(id: &str) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(ItemBuilder {
            client: ClientItemRenderData {
                name: id.to_string(),
                model: ClientItemModel::Texture(String::new()),
            },
            place: None,
            id: Identifier::parse(id).unwrap(),
        }))
    }
    pub fn client_name(this: &mut Arc<Mutex<Self>>, name: &str) -> Arc<Mutex<Self>> {
        this.lock().unwrap().client.name = name.to_string();
        this.clone()
    }
    pub fn client_model_texture(this: &mut Arc<Mutex<Self>>, texture: &str) -> Arc<Mutex<Self>> {
        this.lock().unwrap().client.model = ClientItemModel::Texture(texture.to_string());
        this.clone()
    }
    pub fn client_model_block(this: &mut Arc<Mutex<Self>>, block: &str) -> Arc<Mutex<Self>> {
        this.lock().unwrap().client.model =
            ClientItemModel::Block(Identifier::parse(block).unwrap());
        this.clone()
    }
    pub fn place(this: &mut Arc<Mutex<Self>>, place: &str) -> Arc<Mutex<Self>> {
        this.lock().unwrap().place = Some(Identifier::parse(place).unwrap());
        this.clone()
    }
}
#[derive(Clone)]
pub struct EntityBuilder {
    pub id: Identifier,
    pub client: ClientEntityData,
    pub ticker: Option<FnPtr>,
}
impl EntityBuilder {
    pub fn new(id: &str) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(EntityBuilder {
            id: Identifier::parse(id).unwrap(),
            client: ClientEntityData {
                model: String::new(),
                texture: String::new(),
                hitbox_w: 1.,
                hitbox_h: 1.,
                hitbox_d: 1.,
                animations: Vec::new(),
                items: Vec::new(),
            },
            ticker: None,
        }))
    }
    pub fn tick(this: &mut Arc<Mutex<Self>>, callback: FnPtr) -> Arc<Mutex<Self>> {
        this.lock().unwrap().ticker = Some(callback);
        this.clone()
    }
    pub fn client_model(
        this: &mut Arc<Mutex<Self>>,
        model: &str,
        texture: &str,
    ) -> Arc<Mutex<Self>> {
        {
            let mut borrowed = this.lock().unwrap();
            borrowed.client.model = model.to_string();
            borrowed.client.texture = texture.to_string();
        }
        this.clone()
    }
    pub fn client_hitbox(
        this: &mut Arc<Mutex<Self>>,
        width: f64,
        height: f64,
        depth: f64,
    ) -> Arc<Mutex<Self>> {
        {
            let mut borrowed = this.lock().unwrap();
            borrowed.client.hitbox_w = width as f32;
            borrowed.client.hitbox_h = height as f32;
            borrowed.client.hitbox_d = depth as f32;
        }
        this.clone()
    }
    pub fn client_add_animation(this: &mut Arc<Mutex<Self>>, animation: &str) -> Arc<Mutex<Self>> {
        this.lock()
            .unwrap()
            .client
            .animations
            .push(animation.to_string());
        this.clone()
    }
    pub fn client_add_item(this: &mut Arc<Mutex<Self>>, item: &str) -> Arc<Mutex<Self>> {
        this.lock().unwrap().client.items.push(item.to_string());
        this.clone()
    }
}
#[derive(Clone)]
pub struct ClientContentData {
    pub images: HashMap<Identifier, Vec<u8>, BuildHasherDefault<XxHash64>>,
    pub sounds: HashMap<Identifier, Vec<u8>, BuildHasherDefault<XxHash64>>,
    pub models: HashMap<Identifier, Vec<u8>, BuildHasherDefault<XxHash64>>,
}
impl ClientContentData {
    pub fn new() -> Self {
        ClientContentData {
            images: Default::default(),
            sounds: Default::default(),
            models: Default::default(),
        }
    }
    fn by_type(
        &mut self,
        content_type: ContentType,
    ) -> &mut HashMap<Identifier, Vec<u8>, BuildHasherDefault<XxHash64>> {
        match content_type {
            ContentType::Image => &mut self.images,
            ContentType::Sound => &mut self.sounds,
            ContentType::Model => &mut self.models,
        }
    }
}
#[derive(Clone, Copy)]
enum ContentType {
    Image,
    Sound,
    Model,
}
