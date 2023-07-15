use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc,
    str::FromStr,
    sync::Arc,
};

use anyhow::{bail, Context, Result};

use rhai::{Dynamic, Engine, ImmutableString};
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
        let current_mod_path = rc::Rc::new(RefCell::new(PathBuf::new()));
        let content = rc::Rc::new(RefCell::new(ClientContentData::new()));
        let blocks = rc::Rc::new(RefCell::new(Vec::new()));
        let items = rc::Rc::new(RefCell::new(Vec::new()));
        let entities = rc::Rc::new(RefCell::new(Vec::new()));
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
            .register_fn(
                "register",
                move |this: &mut rc::Rc<RefCell<BlockBuilder>>| {
                    registered_blocks.borrow_mut().push(this.clone())
                },
            );
        loading_engine
            .register_type_with_name::<ItemBuilder>("ItemBuilder")
            .register_fn("create_item", ItemBuilder::new)
            .register_fn("client_name", ItemBuilder::client_name)
            .register_fn("client_model_texture", ItemBuilder::client_model_texture)
            .register_fn("client_model_block", ItemBuilder::client_model_block)
            .register_fn("place", ItemBuilder::place)
            .register_fn(
                "register",
                move |this: &mut rc::Rc<RefCell<ItemBuilder>>| {
                    registered_items.borrow_mut().push(this.clone())
                },
            );
        loading_engine
            .register_type_with_name::<EntityBuilder>("EntityBuilder")
            .register_fn("create_entity", EntityBuilder::new)
            .register_fn("client_model", EntityBuilder::client_model)
            .register_fn("client_hitbox", EntityBuilder::client_hitbox)
            .register_fn("client_add_animation", EntityBuilder::client_add_animation)
            .register_fn("client_add_item", EntityBuilder::client_add_item)
            .register_fn(
                "register",
                move |this: &mut rc::Rc<RefCell<EntityBuilder>>| {
                    registered_entities.borrow_mut().push(this.clone())
                },
            );

        let mut content_register = |name: &str, content_type: ContentType| {
            let register_current_mod_path = current_mod_path.clone();
            let register_content = content.clone();
            loading_engine.register_fn(name, move |id: &str, path: &str| {
                let start_path = { register_current_mod_path.borrow().clone() };
                let mut full_path = start_path.clone();
                full_path.push(path);
                if !full_path.starts_with(start_path) {
                    panic!("path travelsal attack");
                }
                register_content.borrow_mut().by_type(content_type).insert(
                    Identifier::parse(id).unwrap(),
                    std::fs::read(full_path).unwrap(),
                );
            });
        };
        content_register.call_mut(("register_image", ContentType::Image));
        content_register.call_mut(("register_sound", ContentType::Sound));
        content_register.call_mut(("register_model", ContentType::Model));
        for loaded_mod in &mods {
            current_mod_path.replace(loaded_mod.1.path.clone());
            loaded_mod.1.load_scripts(&loading_engine);
        }
        let blocks = blocks
            .borrow()
            .iter()
            .map(|block| block.borrow().clone())
            .collect();
        let items = items
            .borrow()
            .iter()
            .map(|item| item.borrow().clone())
            .collect();
        let entities = entities
            .borrow()
            .iter()
            .map(|entity| entity.borrow().clone())
            .collect();

        //println!("{blocks:#?}\n{items:#?}\n{entities:#?}");
        let content = content.borrow().clone();
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
    pub fn new(id: &str) -> rc::Rc<RefCell<Self>> {
        rc::Rc::new(RefCell::new(BlockBuilder {
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
    pub fn client_type_air(this: &mut rc::Rc<RefCell<Self>>) -> rc::Rc<RefCell<Self>> {
        this.borrow_mut().client.block_type = ClientBlockRenderDataType::Air;
        this.clone()
    }
    pub fn client_type_cube(
        this: &mut rc::Rc<RefCell<Self>>,
        front: &str,
        back: &str,
        right: &str,
        left: &str,
        up: &str,
        down: &str,
    ) -> rc::Rc<RefCell<Self>> {
        this.borrow_mut().client.block_type =
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
    pub fn client_fluid(this: &mut rc::Rc<RefCell<Self>>, fluid: bool) -> rc::Rc<RefCell<Self>> {
        this.borrow_mut().client.fluid = fluid;
        this.clone()
    }
    pub fn client_transparent(
        this: &mut rc::Rc<RefCell<Self>>,
        transparent: bool,
    ) -> rc::Rc<RefCell<Self>> {
        this.borrow_mut().client.transparent = transparent;
        this.clone()
    }
    pub fn client_render_data(
        this: &mut rc::Rc<RefCell<Self>>,
        render_data: i64,
    ) -> rc::Rc<RefCell<Self>> {
        this.borrow_mut().client.render_data = render_data as u8;
        this.clone()
    }
    pub fn client_dynamic(
        this: &mut rc::Rc<RefCell<Self>>,
        model: &str,
        texture: &str,
    ) -> rc::Rc<RefCell<Self>> {
        this.borrow_mut().client.dynamic = Some(ClientBlockDynamicData {
            model: model.to_string(),
            texture: texture.to_string(),
            animations: Vec::new(),
            items: Vec::new(),
        });
        this.clone()
    }
    pub fn client_dynamic_add_animation(
        this: &mut rc::Rc<RefCell<Self>>,
        animation: &str,
    ) -> rc::Rc<RefCell<Self>> {
        //todo: result
        if let Some(dynamic) = &mut this.borrow_mut().client.dynamic {
            dynamic.animations.push(animation.to_string());
        }
        this.clone()
    }
    pub fn client_dynamic_add_item(
        this: &mut rc::Rc<RefCell<Self>>,
        item: &str,
    ) -> rc::Rc<RefCell<Self>> {
        //todo: result
        if let Some(dynamic) = &mut this.borrow_mut().client.dynamic {
            dynamic.items.push(item.to_string());
        }
        this.clone()
    }
}
#[derive(Clone, Debug)]
pub struct ItemBuilder {
    pub id: Identifier,
    pub client: ClientItemRenderData,
    pub place: Option<Identifier>,
}
impl ItemBuilder {
    pub fn new(id: &str) -> rc::Rc<RefCell<Self>> {
        rc::Rc::new(RefCell::new(ItemBuilder {
            client: ClientItemRenderData {
                name: id.to_string(),
                model: ClientItemModel::Texture(String::new()),
            },
            place: None,
            id: Identifier::parse(id).unwrap(),
        }))
    }
    pub fn client_name(this: &mut rc::Rc<RefCell<Self>>, name: &str) -> rc::Rc<RefCell<Self>> {
        this.borrow_mut().client.name = name.to_string();
        this.clone()
    }
    pub fn client_model_texture(
        this: &mut rc::Rc<RefCell<Self>>,
        texture: &str,
    ) -> rc::Rc<RefCell<Self>> {
        this.borrow_mut().client.model = ClientItemModel::Texture(texture.to_string());
        this.clone()
    }
    pub fn client_model_block(
        this: &mut rc::Rc<RefCell<Self>>,
        block: &str,
    ) -> rc::Rc<RefCell<Self>> {
        this.borrow_mut().client.model = ClientItemModel::Block(Identifier::parse(block).unwrap());
        this.clone()
    }
    pub fn place(this: &mut rc::Rc<RefCell<Self>>, place: &str) -> rc::Rc<RefCell<Self>> {
        this.borrow_mut().place = Some(Identifier::parse(place).unwrap());
        this.clone()
    }
}
#[derive(Clone, Debug)]
pub struct EntityBuilder {
    pub id: Identifier,
    pub client: ClientEntityData,
}
impl EntityBuilder {
    pub fn new(id: &str) -> rc::Rc<RefCell<Self>> {
        rc::Rc::new(RefCell::new(EntityBuilder {
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
        }))
    }
    pub fn client_model(
        this: &mut rc::Rc<RefCell<Self>>,
        model: &str,
        texture: &str,
    ) -> rc::Rc<RefCell<Self>> {
        {
            let mut borrowed = this.borrow_mut();
            borrowed.client.model = model.to_string();
            borrowed.client.texture = texture.to_string();
        }
        this.clone()
    }
    pub fn client_hitbox(
        this: &mut rc::Rc<RefCell<Self>>,
        width: f64,
        height: f64,
        depth: f64,
    ) -> rc::Rc<RefCell<Self>> {
        {
            let mut borrowed = this.borrow_mut();
            borrowed.client.hitbox_w = width as f32;
            borrowed.client.hitbox_h = height as f32;
            borrowed.client.hitbox_d = depth as f32;
        }
        this.clone()
    }
    pub fn client_add_animation(
        this: &mut rc::Rc<RefCell<Self>>,
        animation: &str,
    ) -> rc::Rc<RefCell<Self>> {
        this.borrow_mut()
            .client
            .animations
            .push(animation.to_string());
        this.clone()
    }
    pub fn client_add_item(this: &mut rc::Rc<RefCell<Self>>, item: &str) -> rc::Rc<RefCell<Self>> {
        this.borrow_mut().client.items.push(item.to_string());
        this.clone()
    }
}
#[derive(Clone)]
pub struct ClientContentData {
    pub images: HashMap<Identifier, Vec<u8>>,
    pub sounds: HashMap<Identifier, Vec<u8>>,
    pub models: HashMap<Identifier, Vec<u8>>,
}
impl ClientContentData {
    pub fn new() -> Self {
        ClientContentData {
            images: HashMap::new(),
            sounds: HashMap::new(),
            models: HashMap::new(),
        }
    }
    fn by_type(&mut self, content_type: ContentType) -> &mut HashMap<Identifier, Vec<u8>> {
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
