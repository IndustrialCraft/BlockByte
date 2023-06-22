use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use anyhow::{anyhow, bail, Context, Result};
use rlua::{
    Function, Lua, MultiValue, Table, UserData,
    Value::{self, Nil},
};
use tungstenite::stream;
use walkdir::WalkDir;

use crate::{
    registry::{
        Block, BlockRegistry, BlockState, ClientBlockCubeRenderData, ClientBlockRenderData,
        ClientBlockRenderDataType,
    },
    util::Identifier,
};

struct Mod {
    lua: Lua,
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
            lua: {
                let lua = Lua::new();
                lua.context(|ctx| {
                    ctx.set_named_registry_value("mod_id", mod_identifier.clone())
                        .unwrap();
                });
                lua.context(|ctx| {
                    ctx.set_named_registry_value("mod_path", path.to_str().unwrap())
                        .unwrap();
                });
                lua
            },
            path: path.to_path_buf(),
            namespace: mod_identifier,
        })
    }
    pub fn load_scripts(&self) {
        self.lua.context(|ctx| {
            let globals = ctx.globals();
            globals
                .set(
                    "registerEvent",
                    ctx.create_function(|ctx, (event, callback): (rlua::String, Function)| {
                        let event_name = "event_".to_string() + event.to_str().unwrap();
                        let event_list: Value =
                            ctx.named_registry_value(event_name.as_str()).unwrap();
                        let event_list = match event_list {
                            Value::Table(table) => table,
                            _ => ctx.create_table().unwrap(),
                        };
                        event_list
                            .set(event_list.len().unwrap() + 1, callback)
                            .unwrap();
                        println!("registered event {}", event.to_str().unwrap());
                        ctx.set_named_registry_value(event_name.as_str(), event_list)
                            .unwrap();
                        Ok(())
                    })
                    .unwrap(),
                )
                .unwrap();
            for script in WalkDir::new({
                let mut scripts_path = self.path.clone();
                scripts_path.push("scripts");
                scripts_path
            })
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|entry| entry.metadata().unwrap().is_file())
            {
                ctx.load(std::fs::read(script.path()).unwrap().as_slice())
                    .exec()
                    .unwrap();
            }
        });
    }
    pub fn call_event<T>(&self, event: &str, param: T)
    where
        T: UserData,
    {
        self.lua.context(|ctx| {
            ctx.scope(|scope| {
                let event_name = "event_".to_string() + event;
                let event_list: Table = ctx.named_registry_value(event_name.as_str()).unwrap();
                let wrapped = scope.create_nonstatic_userdata(param).unwrap();
                for callback in event_list.sequence_values() {
                    let callback: Function = callback.unwrap();
                    callback.call::<_, ()>(wrapped.clone()).unwrap();
                }
            });
        });
    }
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
    pub fn load_mods(path: &Path) -> Self {
        let mut mods = HashMap::new();
        for mod_path in std::fs::read_dir(path).unwrap() {
            let mod_path = mod_path.unwrap();
            let path = mod_path.path();
            let name = mod_path.file_name().to_str().unwrap().to_string();
            if let Ok(loaded_mod) = Mod::new(path.as_path()) {
                mods.insert(loaded_mod.namespace.clone(), loaded_mod);
            } else {
                println!("loading mod '{}' failed", name);
            }
        }
        for loaded_mod in &mods {
            loaded_mod.1.load_scripts();
        }
        ModManager { mods }
    }
    pub fn call_event<T>(&self, event: &str, param: T)
    where
        T: UserData + Clone,
    {
        for loaded_mod in &self.mods {
            loaded_mod.1.call_event(event, param.clone());
        }
    }
}

#[derive(Clone)]
pub struct BlockRegistryWrapper<'a> {
    pub block_registry: &'a RefCell<BlockRegistry>,
}
impl<'a> UserData for BlockRegistryWrapper<'a> {
    fn add_methods<'lua, T: rlua::UserDataMethods<'lua, Self>>(_methods: &mut T) {
        _methods.add_method("register", |ctx, this, (id, data): (String, Table)| {
            let mod_id: String = ctx.named_registry_value("mod_id").unwrap();

            println!("registered block {}", id);
            this.block_registry
                .borrow_mut()
                .register(Identifier::new(mod_id, id), |client_id| {
                    let block = Arc::new(Block {
                        default_state: client_id,
                    });
                    let state = BlockState {
                        state_id: client_id,
                        client_data: BlockRegistryWrapper::client_data_from_table(
                            data.get("client").unwrap(),
                        ),
                        parent: block.clone(),
                    };
                    (block, vec![state])
                })
                .unwrap();
            Ok(())
        })
    }
}
impl<'a> BlockRegistryWrapper<'a> {
    fn client_data_from_table(table: Table) -> ClientBlockRenderData {
        ClientBlockRenderData {
            block_type: BlockRegistryWrapper::client_render_type_from_table(
                table.get("render_type").unwrap(),
            ),
            fluid: table.get("fluid").unwrap_or(false),
            render_data: table.get("render_data").unwrap_or(0),
            transparent: table.get("transparent").unwrap_or(false),
        }
    }
    fn client_render_type_from_table(table: Table) -> ClientBlockRenderDataType {
        let render_type: String = table.get("type").unwrap();
        match render_type.as_str() {
            "air" => ClientBlockRenderDataType::Air,
            "cube" => ClientBlockRenderDataType::Cube(ClientBlockCubeRenderData {
                front: table.get("front").unwrap(),
                back: table.get("back").unwrap(),
                right: table.get("right").unwrap(),
                left: table.get("left").unwrap(),
                up: table.get("up").unwrap(),
                down: table.get("down").unwrap(),
            }),
            _ => unimplemented!(),
        }
    }
}
pub struct ClientContentData {
    pub images: HashMap<Identifier, Vec<u8>>,
    pub sounds: HashMap<Identifier, Vec<u8>>,
}
#[derive(Clone)]
pub struct ClientContentDataWrapper<'a> {
    pub client_content: &'a RefCell<ClientContentData>,
}
impl ClientContentData {
    pub fn new() -> Self {
        ClientContentData {
            images: HashMap::new(),
            sounds: HashMap::new(),
        }
    }
}
impl<'a> UserData for ClientContentDataWrapper<'a> {
    fn add_methods<'lua, T: rlua::UserDataMethods<'lua, Self>>(_methods: &mut T) {
        _methods.add_method(
            "register_image",
            |ctx, this, (id, path): (String, String)| {
                let mod_id: String = ctx.named_registry_value("mod_id").unwrap();
                let mod_path: String = ctx.named_registry_value("mod_path").unwrap();
                let id = Identifier {
                    namespace: mod_id,
                    key: id,
                };
                this.client_content.borrow_mut().images.insert(
                    id,
                    std::fs::read(
                        PathBuf::from_str(mod_path.as_str())
                            .unwrap()
                            .join(Path::new(path.as_str())),
                    ) //todo: fix directory travelsal attack
                    .unwrap(),
                );
                Ok(())
            },
        );
        _methods.add_method(
            "register_sound",
            |ctx, this, (id, path): (String, String)| {
                let mod_id: String = ctx.named_registry_value("mod_id").unwrap();
                let mod_path: String = ctx.named_registry_value("mod_path").unwrap();
                let id = Identifier {
                    namespace: mod_id,
                    key: id,
                };
                this.client_content.borrow_mut().sounds.insert(
                    id,
                    std::fs::read(
                        PathBuf::from_str(mod_path.as_str())
                            .unwrap()
                            .join(Path::new(path.as_str())),
                    ) //todo: fix directory travelsal attack
                    .unwrap(),
                );
                Ok(())
            },
        );
    }
}
