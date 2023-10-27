#![feature(fn_traits)]
#![feature(map_many_mut)]
#![feature(hash_extract_if)]
mod content;
mod game;
mod gui;
mod model;
mod net;
mod render;
mod texture;

use array_init::array_init;
use block_byte_common::messages::{ClientModelTarget, NetworkMessageC2S, NetworkMessageS2C};
use block_byte_common::{BlockPosition, Face, KeyboardKey, KeyboardModifier, Position, AABB};
use cgmath::Point3;
use std::collections::{HashMap, HashSet};
use std::env::args;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;
use winit::dpi::PhysicalPosition;
use winit::event::ElementState::Pressed;
use winit::window::CursorGrabMode;
use winit::{
    event::*,
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

use crate::game::{ClientPlayer, EntityData, RaycastResult, World};
use crate::gui::GUIRenderer;
use crate::model::ModelInstanceData;
use crate::net::SocketConnection;
use crate::render::RenderState;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg_attr(target_arch = "wasm32", wasm_bindgen(start))]
pub async fn run() {
    cfg_if::cfg_if! {
        if #[cfg(target_arch = "wasm32")] {
            std::panic::set_hook(Box::new(console_error_panic_hook::hook));
            console_log::init_with_level(log::Level::Warn).expect("Couldn't initialize logger");
        } else {
            env_logger::init();
        }
    }
    let args: Vec<String> = args().collect();
    let (
        texture_image,
        texture_atlas,
        block_registry,
        item_registry,
        entity_registry,
        text_renderer,
        mut sound_manager,
    ) = content::load_assets(PathBuf::from(args.get(1).unwrap()), false);
    let block_registry = Rc::new(block_registry);
    let entity_registry = Rc::new(entity_registry);

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new().build(&event_loop).unwrap();
    window.set_cursor_grab(CursorGrabMode::Confined).ok();
    window.set_cursor_visible(false);
    #[cfg(target_arch = "wasm32")]
    {
        use winit::dpi::PhysicalSize;

        use winit::platform::web::WindowExtWebSys;
        web_sys::window()
            .and_then(|win| win.document())
            .and_then(|doc| {
                let dst = doc.get_element_by_id("wasm-example")?;
                let canvas = web_sys::Element::from(window.canvas());
                dst.append_child(&canvas).ok()?;
                Some(())
            })
            .expect("Couldn't append canvas to document body.");
    }
    let mut render_state = RenderState::new(window, texture_image).await;
    let mut camera = ClientPlayer::at_position(
        Position {
            x: 0.,
            y: 0.,
            z: 0.,
        },
        block_registry.clone(),
    );
    let mut keys = HashSet::new();
    let mut world = World::new(block_registry.clone(), entity_registry.clone());
    let mut gui = GUIRenderer::new(texture_atlas, render_state.device(), text_renderer);
    let mut connection = SocketConnection::new(args.get(2).unwrap());
    let mut first_teleport = false;
    let mut last_render_time = Instant::now();
    let mut fluid_selectable = false;

    let mut last_position_sent = Instant::now();

    let mut block_breaking_manager = BlockBreakingManager::new();

    let mut player_entity_type = None;

    let text_input_channel = spawn_stdin_channel();

    let mut viewmodel_instance = ModelInstanceData::new();
    #[allow(deprecated)]
    event_loop.run(move |event, _, control_flow| match event {
        Event::WindowEvent {
            ref event,
            window_id,
        } if window_id == render_state.window().id() => match event {
            WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
            WindowEvent::KeyboardInput {
                input:
                    KeyboardInput {
                        state,
                        virtual_keycode,
                        modifiers: mods,
                        ..
                    },
                ..
            } => {
                if let Some(virtual_keycode) = virtual_keycode.as_ref() {
                    match state {
                        ElementState::Pressed => {
                            keys.insert(*virtual_keycode);
                        }
                        ElementState::Released => {
                            keys.remove(virtual_keycode);
                        }
                    }
                    let mut modifiers = 0;
                    if mods.contains(ModifiersState::SHIFT) {
                        modifiers |= KeyboardModifier::SHIFT;
                    }
                    if mods.contains(ModifiersState::CTRL) {
                        modifiers |= KeyboardModifier::CTRL;
                    }
                    if mods.contains(ModifiersState::ALT) {
                        modifiers |= KeyboardModifier::ALT;
                    }
                    connection.send_message(&NetworkMessageC2S::Keyboard(
                        keyboard_key_from_virtual_keycode(*virtual_keycode),
                        modifiers,
                        *state == ElementState::Pressed,
                        false,
                    ));
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if !gui.is_cursor_locked() {
                    if let Some(element) = gui.get_selected(render_state.mouse, render_state.size())
                    {
                        if *state == ElementState::Pressed {
                            connection.send_message(&NetworkMessageC2S::GuiClick(
                                element.0.to_string(),
                                match button {
                                    MouseButton::Left => {
                                        block_byte_common::messages::MouseButton::Left
                                    }
                                    MouseButton::Right => {
                                        block_byte_common::messages::MouseButton::Right
                                    }

                                    MouseButton::Middle => {
                                        block_byte_common::messages::MouseButton::Middle
                                    }

                                    MouseButton::Other(n) => {
                                        block_byte_common::messages::MouseButton::Other(*n)
                                    }
                                },
                                keys.contains(&VirtualKeyCode::LShift),
                            ));
                        }
                    }
                } else {
                    if *button == MouseButton::Left {
                        block_breaking_manager.set_left_click_held(*state == Pressed);
                    }
                    match world.raycast(5., camera.get_eye(), camera.make_front(), fluid_selectable)
                    {
                        RaycastResult::Entity(id) => {
                            if *state == ElementState::Pressed {
                                match button {
                                    MouseButton::Left => connection
                                        .send_message(&NetworkMessageC2S::LeftClickEntity(id)),
                                    MouseButton::Right => connection
                                        .send_message(&NetworkMessageC2S::RightClickEntity(id)),
                                    _ => {}
                                }
                            }
                        }
                        RaycastResult::Block(position, face) => match button {
                            MouseButton::Right => {
                                if *state == ElementState::Pressed {
                                    connection.send_message(&NetworkMessageC2S::RightClickBlock(
                                        position,
                                        face,
                                        camera.is_shifting(),
                                    ))
                                }
                            }
                            _ => {}
                        },
                        RaycastResult::Miss => {}
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => match delta {
                MouseScrollDelta::LineDelta(x, y) => {
                    let x = *x as i32;
                    let y = *y as i32;
                    if gui.is_cursor_locked() {
                        connection.send_message(&NetworkMessageC2S::MouseScroll(x, y));
                    } else {
                        if let Some(element) =
                            gui.get_selected(render_state.mouse, render_state.size())
                        {
                            connection.send_message(&NetworkMessageC2S::GuiScroll(
                                element.0.to_string(),
                                x,
                                y,
                                keys.contains(&VirtualKeyCode::LShift),
                            ));
                        }
                    }
                }
                MouseScrollDelta::PixelDelta(_) => {}
            },
            WindowEvent::Resized(physical_size) => {
                render_state.resize(*physical_size);
            }
            WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                render_state.resize(**new_inner_size);
            }
            WindowEvent::CursorMoved { position, .. } => {
                render_state.mouse = *position;
            }
            _ => {}
        },
        Event::DeviceEvent {
            ref event,
            device_id: _,
        } => match event {
            DeviceEvent::MouseMotion { delta: (x, y) } => {
                if gui.is_cursor_locked() {
                    let sensitivity = 0.3;
                    camera.update_orientation(-*y as f32 * sensitivity, -*x as f32 * sensitivity);
                }
            }
            _ => {}
        },
        Event::RedrawRequested(window_id) if window_id == render_state.window().id() => {
            let now = Instant::now();
            let dt = now - last_render_time;
            last_render_time = now;
            let dt = dt.as_secs_f32();
            camera.update_position(&keys, dt, &world);
            render_state.window().set_title(&format!(
                "BlockByte x: {:.1} y: {:.1} z: {:.1} fps: {:.0} {}",
                camera.position.x,
                camera.position.y,
                camera.position.z,
                1. / dt,
                block_breaking_manager
                    .breaking_animation
                    .as_ref()
                    .map(|animation| format!(
                        "breaking: {}%",
                        (animation.0 / animation.1 * 100.) as u8
                    ))
                    .unwrap_or(String::new())
            ));
            if let Some(animation) = viewmodel_instance.animation.as_mut() {
                animation.1 += dt;
            }
            block_breaking_manager.tick(dt, &mut connection, keys.contains(&VirtualKeyCode::R));
            while let Ok(message) = text_input_channel.try_recv() {
                connection.send_message(&NetworkMessageC2S::SendMessage(message));
            }
            let raycast =
                world.raycast(5., camera.get_eye(), camera.make_front(), fluid_selectable);
            block_breaking_manager.set_target_block(match raycast {
                RaycastResult::Block(block, face) => Some((block, face)),
                _ => None,
            });
            render_state.outline_renderer.set_aabb(
                match raycast {
                    RaycastResult::Entity(id) => {
                        let entity = world.entities.get(&id).unwrap();
                        let position = entity.position;
                        let entity_data = entity_registry.get_entity(entity.type_id);
                        Some(AABB {
                            x: position.x,
                            y: position.y,
                            z: position.z,
                            w: entity_data.hitbox_w,
                            h: entity_data.hitbox_h,
                            d: entity_data.hitbox_d,
                        })
                    }
                    RaycastResult::Block(position, _) => Some(AABB {
                        x: position.x as f64,
                        y: position.y as f64,
                        z: position.z as f64,
                        w: 1.,
                        h: 1.,
                        d: 1.,
                    }),
                    RaycastResult::Miss => None,
                },
                &render_state.queue,
            );
            for (_, dynamic_block_data) in &mut world.dynamic_blocks {
                if let Some(animation) = dynamic_block_data.model_instance.animation.as_mut() {
                    animation.1 += dt;
                    animation.1 %= block_registry
                        .get_block(dynamic_block_data.id)
                        .dynamic
                        .as_ref()
                        .unwrap()
                        .get_animation_length(animation.0)
                        .unwrap_or(0.);
                }
            }
            if first_teleport && last_position_sent.elapsed().as_millis() > 100 {
                last_position_sent = Instant::now();
                connection.send_message(&NetworkMessageC2S::PlayerPosition(
                    Position {
                        x: camera.position.x as f64,
                        y: camera.position.y as f64,
                        z: camera.position.z as f64,
                    },
                    camera.is_shifting(),
                    camera.yaw_deg,
                    camera.last_moved,
                ));
            }
            for message in connection.read_messages() {
                match message {
                    NetworkMessageS2C::SetBlock(block_position, id) => {
                        world.set_block(block_position, id);
                    }
                    NetworkMessageS2C::LoadChunk(position, palette, blocks) => {
                        let mut decoder = flate2::read::GzDecoder::new(blocks.as_slice());
                        let mut blocks_data = Vec::new();
                        std::io::copy(&mut decoder, &mut blocks_data).unwrap();
                        let blocks: [[[u16; 16]; 16]; 16] =
                            bitcode::deserialize(blocks_data.as_slice()).unwrap();
                        let blocks = array_init(|x| {
                            array_init(|y| {
                                array_init(|z| *palette.get(blocks[x][y][z] as usize).unwrap())
                            })
                        });
                        world.load_chunk(position, blocks)
                    }
                    NetworkMessageS2C::UnloadChunk(position) => {
                        world.unload_chunk(position);
                    }
                    NetworkMessageS2C::GuiSetElement(id, element) => {
                        gui.set_element(id, element);
                    }
                    NetworkMessageS2C::GuiRemoveElements(id) => {
                        gui.remove_elements(id.as_str());
                    }
                    NetworkMessageS2C::GuiEditElement(id, edit) => {
                        if let Some(element) = gui.get_element(id) {
                            element.edit(edit);
                        }
                    }
                    NetworkMessageS2C::SetCursorLock(locked) => {
                        gui.set_cursor_locked(locked);
                        render_state
                            .window()
                            .set_cursor_grab(if locked {
                                CursorGrabMode::Confined
                            } else {
                                CursorGrabMode::None
                            })
                            .ok();
                        render_state.window().set_cursor_visible(!locked);
                        render_state
                            .window()
                            .set_cursor_position(PhysicalPosition {
                                x: render_state.size().width as f32 / 2.,
                                y: render_state.size().height as f32 / 2.,
                            })
                            .ok();
                    }
                    NetworkMessageS2C::AddEntity(type_id, id, position, rotation, animation, _) => {
                        world.entities.insert(
                            id,
                            EntityData {
                                type_id,
                                position,
                                rotation,
                                model_instance: ModelInstanceData {
                                    items: HashMap::new(),
                                    animation: Some((animation, 0.)),
                                },
                            },
                        );
                    }
                    NetworkMessageS2C::MoveEntity(id, position, rotation) => {
                        if let Some(entity) = world.entities.get_mut(&id) {
                            entity.position = position;
                            entity.rotation = rotation;
                        }
                    }
                    NetworkMessageS2C::DeleteEntity(id) => {
                        world.entities.remove(&id);
                    }
                    NetworkMessageS2C::BlockBreakTimeResponse(id, time) => {
                        block_breaking_manager.on_block_break_time_response(id, time);
                    }
                    NetworkMessageS2C::Knockback(x, y, z, set) => {
                        camera.knockback(x, y, z, set);
                    }
                    NetworkMessageS2C::FluidSelectable(selectable) => {
                        fluid_selectable = selectable;
                    }
                    NetworkMessageS2C::PlaySound(id, position, gain, pitch, relative) => {
                        sound_manager.play_sound(id.as_str(), position, gain, pitch, relative);
                    }
                    NetworkMessageS2C::ChatMessage(message) => {
                        println!("[CHAT]{}", message);
                    }
                    NetworkMessageS2C::PlayerAbilities(speed, movement_type) => {
                        camera.set_abilities(speed, movement_type);
                    }
                    NetworkMessageS2C::TeleportPlayer(position, rotation) => {
                        camera.position =
                            Point3::new(position.x as f32, position.y as f32, position.z as f32);
                        camera.pitch_deg = rotation;
                        first_teleport = true;
                    }
                    NetworkMessageS2C::ModelAnimation(target, animation) => {
                        let model_instance = match target {
                            ClientModelTarget::Block(position) => world
                                .get_dynamic_block_data(position)
                                .map(|block| &mut block.model_instance),
                            ClientModelTarget::Entity(id) => world
                                .entities
                                .get_mut(&id)
                                .map(|entity| &mut entity.model_instance),
                            ClientModelTarget::ViewModel => Some(&mut viewmodel_instance),
                        };
                        if let Some(model_instance) = model_instance {
                            model_instance.animation = Some((animation, 0.));
                        }
                    }
                    NetworkMessageS2C::ModelItem(target, slot, item) => {
                        let model_data = match target {
                            ClientModelTarget::Block(position) => {
                                let block_id = world.get_block(position);
                                world.get_dynamic_block_data(position).map(|block| {
                                    (
                                        &mut block.model_instance,
                                        block_registry
                                            .get_block(block_id.unwrap())
                                            .dynamic
                                            .as_ref()
                                            .and_then(|model| model.get_item_slot(slot))
                                            .unwrap(),
                                    )
                                })
                            }
                            ClientModelTarget::Entity(id) => {
                                world.entities.get_mut(&id).map(|entity| {
                                    (
                                        &mut entity.model_instance,
                                        entity_registry
                                            .get_entity(entity.type_id)
                                            .model
                                            .get_item_slot(slot)
                                            .unwrap(),
                                    )
                                })
                            }
                            ClientModelTarget::ViewModel => player_entity_type
                                .as_ref()
                                .map(|id| entity_registry.get_entity(*id))
                                .and_then(|entity| entity.viewmodel.as_ref())
                                .map(|viewmodel| {
                                    (
                                        &mut viewmodel_instance,
                                        viewmodel.get_item_slot(slot).unwrap(),
                                    )
                                }),
                        };
                        if let Some((model_instance, slot)) = model_data {
                            match item {
                                Some(item) => {
                                    model_instance.items.insert(slot.clone(), item);
                                }
                                None => {
                                    model_instance.items.remove(slot);
                                }
                            }
                        }
                    }
                    NetworkMessageS2C::ControllingEntity(id) => {
                        player_entity_type = Some(id);
                        camera.hitbox = player_entity_type.as_ref().map(|id| {
                            let entity = entity_registry.get_entity(*id);
                            (
                                entity.hitbox_w,
                                entity.hitbox_h,
                                entity.hitbox_d,
                                entity.hitbox_h_shifting,
                            )
                        });
                        viewmodel_instance = ModelInstanceData::new();
                    }
                }
            }
            match render_state.render(
                &camera,
                &mut world,
                &mut gui,
                &item_registry,
                &entity_registry,
                player_entity_type
                    .as_ref()
                    .map(|id| entity_registry.get_entity(*id))
                    .and_then(|entity| entity.viewmodel.as_ref())
                    .map(|model| (model, &viewmodel_instance)),
            ) {
                Ok(_) => {}
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    render_state.resize(render_state.size())
                }
                Err(wgpu::SurfaceError::OutOfMemory) => *control_flow = ControlFlow::Exit,

                Err(wgpu::SurfaceError::Timeout) => log::warn!("Surface timeout"),
            }
        }
        Event::RedrawEventsCleared => {
            render_state.window().request_redraw();
        }
        _ => {}
    })
}
struct BlockBreakingManager {
    id: u32,
    time_requested: bool,
    target_block: Option<(BlockPosition, Face)>,
    key_down: bool,
    breaking_animation: Option<(f32, f32)>,
    just_pressed: bool,
}
impl BlockBreakingManager {
    pub fn new() -> Self {
        BlockBreakingManager {
            id: 0,
            target_block: None,
            breaking_animation: None,
            key_down: false,
            time_requested: false,
            just_pressed: false,
        }
    }
    pub fn tick(
        &mut self,
        delta_time: f32,
        connection: &mut SocketConnection,
        keep_breaking: bool,
    ) {
        if let Some(target_block) = self.target_block {
            if self.key_down
                && self.breaking_animation.is_none()
                && !self.time_requested
                && (keep_breaking || self.just_pressed)
            {
                self.time_requested = true;
                self.id += 1;
                connection.send_message(&NetworkMessageC2S::RequestBlockBreakTime(
                    self.id,
                    target_block.0,
                ));
            }
        }
        if let Some(breaking_animation) = &mut self.breaking_animation {
            if let Some(target_block) = self.target_block {
                breaking_animation.0 += delta_time;
                if breaking_animation.0 >= breaking_animation.1 {
                    self.breaking_animation = None;
                    connection.send_message(&NetworkMessageC2S::BreakBlock(target_block.0));
                }
            }
        }
        self.just_pressed = false;
    }
    pub fn on_block_break_time_response(&mut self, id: u32, time: f32) {
        if self.id == id {
            self.breaking_animation = Some((0., time));
            self.time_requested = false;
        }
    }
    pub fn set_left_click_held(&mut self, held: bool) {
        if (!self.key_down) && held {
            self.just_pressed = true;
        }
        self.time_requested = false;
        self.key_down = held;
        if !held {
            self.breaking_animation = None;
        }
    }
    pub fn set_target_block(&mut self, block: Option<(BlockPosition, Face)>) {
        if match (self.target_block, block) {
            (Some(previous), Some(current)) => previous.0 != current.0,
            _ => true,
        } {
            self.breaking_animation = None;
            self.time_requested = false;
        }
        self.target_block = block;
    }
}

fn spawn_stdin_channel() -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || loop {
        let mut buffer = String::new();
        std::io::stdin().read_line(&mut buffer).unwrap();
        tx.send(buffer).unwrap();
    });
    rx
}
pub fn keyboard_key_from_virtual_keycode(keycode: VirtualKeyCode) -> KeyboardKey {
    match keycode {
        VirtualKeyCode::Key1 => KeyboardKey::Key1,
        VirtualKeyCode::Key2 => KeyboardKey::Key2,
        VirtualKeyCode::Key3 => KeyboardKey::Key3,
        VirtualKeyCode::Key4 => KeyboardKey::Key4,
        VirtualKeyCode::Key5 => KeyboardKey::Key5,
        VirtualKeyCode::Key6 => KeyboardKey::Key6,
        VirtualKeyCode::Key7 => KeyboardKey::Key7,
        VirtualKeyCode::Key8 => KeyboardKey::Key8,
        VirtualKeyCode::Key9 => KeyboardKey::Key9,
        VirtualKeyCode::Key0 => KeyboardKey::Key0,
        VirtualKeyCode::A => KeyboardKey::A,
        VirtualKeyCode::B => KeyboardKey::B,
        VirtualKeyCode::C => KeyboardKey::C,
        VirtualKeyCode::D => KeyboardKey::D,
        VirtualKeyCode::E => KeyboardKey::E,
        VirtualKeyCode::F => KeyboardKey::F,
        VirtualKeyCode::G => KeyboardKey::G,
        VirtualKeyCode::H => KeyboardKey::H,
        VirtualKeyCode::I => KeyboardKey::I,
        VirtualKeyCode::J => KeyboardKey::J,
        VirtualKeyCode::K => KeyboardKey::K,
        VirtualKeyCode::L => KeyboardKey::L,
        VirtualKeyCode::M => KeyboardKey::M,
        VirtualKeyCode::N => KeyboardKey::N,
        VirtualKeyCode::O => KeyboardKey::O,
        VirtualKeyCode::P => KeyboardKey::P,
        VirtualKeyCode::Q => KeyboardKey::Q,
        VirtualKeyCode::R => KeyboardKey::R,
        VirtualKeyCode::S => KeyboardKey::S,
        VirtualKeyCode::T => KeyboardKey::T,
        VirtualKeyCode::U => KeyboardKey::U,
        VirtualKeyCode::V => KeyboardKey::V,
        VirtualKeyCode::W => KeyboardKey::W,
        VirtualKeyCode::X => KeyboardKey::X,
        VirtualKeyCode::Y => KeyboardKey::Y,
        VirtualKeyCode::Z => KeyboardKey::Z,
        VirtualKeyCode::F1 => KeyboardKey::F1,
        VirtualKeyCode::F2 => KeyboardKey::F2,
        VirtualKeyCode::F3 => KeyboardKey::F3,
        VirtualKeyCode::F4 => KeyboardKey::F4,
        VirtualKeyCode::F5 => KeyboardKey::F5,
        VirtualKeyCode::F6 => KeyboardKey::F6,
        VirtualKeyCode::F7 => KeyboardKey::F7,
        VirtualKeyCode::F8 => KeyboardKey::F8,
        VirtualKeyCode::F9 => KeyboardKey::F9,
        VirtualKeyCode::F10 => KeyboardKey::F10,
        VirtualKeyCode::F11 => KeyboardKey::F11,
        VirtualKeyCode::F12 => KeyboardKey::F12,
        VirtualKeyCode::F13 => KeyboardKey::F13,
        VirtualKeyCode::F14 => KeyboardKey::F14,
        VirtualKeyCode::F15 => KeyboardKey::F15,
        VirtualKeyCode::F16 => KeyboardKey::F16,
        VirtualKeyCode::F17 => KeyboardKey::F17,
        VirtualKeyCode::F18 => KeyboardKey::F18,
        VirtualKeyCode::F19 => KeyboardKey::F19,
        VirtualKeyCode::F20 => KeyboardKey::F20,
        VirtualKeyCode::F21 => KeyboardKey::F21,
        VirtualKeyCode::F22 => KeyboardKey::F22,
        VirtualKeyCode::F23 => KeyboardKey::F23,
        VirtualKeyCode::F24 => KeyboardKey::F24,
        VirtualKeyCode::Snapshot => KeyboardKey::Snapshot,
        VirtualKeyCode::Scroll => KeyboardKey::Scroll,
        VirtualKeyCode::Pause => KeyboardKey::Pause,
        VirtualKeyCode::Insert => KeyboardKey::Insert,
        VirtualKeyCode::Home => KeyboardKey::Home,
        VirtualKeyCode::Delete => KeyboardKey::Delete,
        VirtualKeyCode::End => KeyboardKey::End,
        VirtualKeyCode::PageDown => KeyboardKey::PageDown,
        VirtualKeyCode::PageUp => KeyboardKey::PageUp,
        VirtualKeyCode::Left => KeyboardKey::Left,
        VirtualKeyCode::Up => KeyboardKey::Up,
        VirtualKeyCode::Right => KeyboardKey::Right,
        VirtualKeyCode::Down => KeyboardKey::Down,
        VirtualKeyCode::Back => KeyboardKey::Backspace,
        VirtualKeyCode::Return => KeyboardKey::Enter,
        VirtualKeyCode::Space => KeyboardKey::Space,
        VirtualKeyCode::Compose => KeyboardKey::Compose,
        VirtualKeyCode::Caret => KeyboardKey::Caret,
        VirtualKeyCode::Numlock => KeyboardKey::Numlock,
        VirtualKeyCode::Numpad0 => KeyboardKey::Numpad0,
        VirtualKeyCode::Numpad1 => KeyboardKey::Numpad1,
        VirtualKeyCode::Numpad2 => KeyboardKey::Numpad2,
        VirtualKeyCode::Numpad3 => KeyboardKey::Numpad3,
        VirtualKeyCode::Numpad4 => KeyboardKey::Numpad4,
        VirtualKeyCode::Numpad5 => KeyboardKey::Numpad5,
        VirtualKeyCode::Numpad6 => KeyboardKey::Numpad6,
        VirtualKeyCode::Numpad7 => KeyboardKey::Numpad7,
        VirtualKeyCode::Numpad8 => KeyboardKey::Numpad8,
        VirtualKeyCode::Numpad9 => KeyboardKey::Numpad9,
        VirtualKeyCode::NumpadAdd => KeyboardKey::NumpadAdd,
        VirtualKeyCode::NumpadDivide => KeyboardKey::NumpadDivide,
        VirtualKeyCode::NumpadDecimal => KeyboardKey::NumpadDecimal,
        VirtualKeyCode::NumpadComma => KeyboardKey::NumpadComma,
        VirtualKeyCode::NumpadEnter => KeyboardKey::NumpadEnter,
        VirtualKeyCode::NumpadEquals => KeyboardKey::NumpadEquals,
        VirtualKeyCode::NumpadMultiply => KeyboardKey::NumpadMultiply,
        VirtualKeyCode::NumpadSubtract => KeyboardKey::NumpadSubtract,
        VirtualKeyCode::AbntC1 => KeyboardKey::AbntC1,
        VirtualKeyCode::AbntC2 => KeyboardKey::AbntC2,
        VirtualKeyCode::Apostrophe => KeyboardKey::Apostrophe,
        VirtualKeyCode::Apps => KeyboardKey::Apps,
        VirtualKeyCode::Asterisk => KeyboardKey::Asterisk,
        VirtualKeyCode::At => KeyboardKey::At,
        VirtualKeyCode::Ax => KeyboardKey::Ax,
        VirtualKeyCode::Backslash => KeyboardKey::Backslash,
        VirtualKeyCode::Calculator => KeyboardKey::Calculator,
        VirtualKeyCode::Capital => KeyboardKey::Capital,
        VirtualKeyCode::Colon => KeyboardKey::Colon,
        VirtualKeyCode::Comma => KeyboardKey::Comma,
        VirtualKeyCode::Convert => KeyboardKey::Convert,
        VirtualKeyCode::Equals => KeyboardKey::Equals,
        VirtualKeyCode::Grave => KeyboardKey::Grave,
        VirtualKeyCode::Kana => KeyboardKey::Kana,
        VirtualKeyCode::Kanji => KeyboardKey::Kanji,
        VirtualKeyCode::LAlt => KeyboardKey::LAlt,
        VirtualKeyCode::LBracket => KeyboardKey::LBracket,
        VirtualKeyCode::LControl => KeyboardKey::LControl,
        VirtualKeyCode::LShift => KeyboardKey::LShift,
        VirtualKeyCode::LWin => KeyboardKey::LWin,
        VirtualKeyCode::Mail => KeyboardKey::Mail,
        VirtualKeyCode::MediaSelect => KeyboardKey::MediaSelect,
        VirtualKeyCode::MediaStop => KeyboardKey::MediaStop,
        VirtualKeyCode::Minus => KeyboardKey::Minus,
        VirtualKeyCode::Mute => KeyboardKey::Mute,
        VirtualKeyCode::MyComputer => KeyboardKey::MyComputer,
        VirtualKeyCode::NavigateForward => KeyboardKey::NavigateForward,
        VirtualKeyCode::NavigateBackward => KeyboardKey::NavigateBackward,
        VirtualKeyCode::NextTrack => KeyboardKey::NextTrack,
        VirtualKeyCode::NoConvert => KeyboardKey::NoConvert,
        VirtualKeyCode::OEM102 => KeyboardKey::OEM102,
        VirtualKeyCode::Period => KeyboardKey::Period,
        VirtualKeyCode::PlayPause => KeyboardKey::PlayPause,
        VirtualKeyCode::Plus => KeyboardKey::Plus,
        VirtualKeyCode::Power => KeyboardKey::Power,
        VirtualKeyCode::PrevTrack => KeyboardKey::PrevTrack,
        VirtualKeyCode::RAlt => KeyboardKey::RAlt,
        VirtualKeyCode::RBracket => KeyboardKey::RBracket,
        VirtualKeyCode::RControl => KeyboardKey::RControl,
        VirtualKeyCode::RShift => KeyboardKey::RShift,
        VirtualKeyCode::RWin => KeyboardKey::RWin,
        VirtualKeyCode::Semicolon => KeyboardKey::Semicolon,
        VirtualKeyCode::Slash => KeyboardKey::Slash,
        VirtualKeyCode::Sleep => KeyboardKey::Sleep,
        VirtualKeyCode::Stop => KeyboardKey::Stop,
        VirtualKeyCode::Sysrq => KeyboardKey::Sysrq,
        VirtualKeyCode::Tab => KeyboardKey::Tab,
        VirtualKeyCode::Underline => KeyboardKey::Underline,
        VirtualKeyCode::Unlabeled => KeyboardKey::Unlabeled,
        VirtualKeyCode::VolumeDown => KeyboardKey::VolumeDown,
        VirtualKeyCode::VolumeUp => KeyboardKey::VolumeUp,
        VirtualKeyCode::Wake => KeyboardKey::Wake,
        VirtualKeyCode::WebBack => KeyboardKey::WebBack,
        VirtualKeyCode::WebFavorites => KeyboardKey::WebFavorites,
        VirtualKeyCode::WebForward => KeyboardKey::WebForward,
        VirtualKeyCode::WebHome => KeyboardKey::WebHome,
        VirtualKeyCode::WebRefresh => KeyboardKey::WebRefresh,
        VirtualKeyCode::WebSearch => KeyboardKey::WebSearch,
        VirtualKeyCode::WebStop => KeyboardKey::WebStop,
        VirtualKeyCode::Yen => KeyboardKey::Yen,
        VirtualKeyCode::Copy => KeyboardKey::Copy,
        VirtualKeyCode::Paste => KeyboardKey::Paste,
        VirtualKeyCode::Cut => KeyboardKey::Cut,
        VirtualKeyCode::Escape => KeyboardKey::Escape,
    }
}
