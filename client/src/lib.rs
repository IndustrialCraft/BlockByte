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
use block_byte_common::gui::{GUIComponent, GUIElement, PositionAnchor};
use block_byte_common::messages::{NetworkMessageC2S, NetworkMessageS2C};
use block_byte_common::{ChunkPosition, Color, KeyboardKey, Position, Vec2};
use cgmath::Point3;
use std::collections::HashSet;
use std::io::repeat;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;
use winit::dpi::{LogicalPosition, PhysicalPosition};
use winit::window::CursorGrabMode;
use winit::{
    event::*,
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

use crate::game::{ClientPlayer, World};
use crate::gui::GUIRenderer;
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
    let (texture_image, texture_atlas, block_registry, item_registry) =
        content::load_assets(&Path::new("../server/save/content.zip"));
    let block_registry = Rc::new(block_registry);

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new().build(&event_loop).unwrap();
    window.set_cursor_grab(CursorGrabMode::Confined).unwrap();
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
    let mut camera = ClientPlayer::at_position(Position {
        x: 0.,
        y: 0.,
        z: 0.,
    });
    let mut keys = HashSet::new();
    let mut world = World::new(block_registry.clone());
    let mut gui = GUIRenderer::new(texture_atlas, render_state.device());
    let mut connection = SocketConnection::new("localhost:4321");
    let mut first_teleport = false;
    let mut last_render_time = Instant::now();
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
                    connection.send_message(&NetworkMessageC2S::Keyboard(
                        keyboard_key_from_virtual_keycode(*virtual_keycode),
                        0,
                        *state == ElementState::Pressed,
                        false,
                    ))
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if *state == ElementState::Pressed && !gui.is_cursor_locked() {
                    if let Some(element) = gui.get_selected(render_state.mouse, render_state.size())
                    {
                        connection.send_message(&NetworkMessageC2S::GuiClick(
                            element,
                            match button {
                                MouseButton::Left => block_byte_common::messages::MouseButton::Left,
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
                                element,
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
                    camera.update_orientation(-*y as f32, -*x as f32);
                }
            }
            _ => {}
        },
        Event::RedrawRequested(window_id) if window_id == render_state.window().id() => {
            let now = Instant::now();
            let dt = now - last_render_time;
            last_render_time = now;
            let dt = dt.as_secs_f32();
            camera.update_position(&keys, dt);
            render_state.window().set_title(&format!(
                "BlockByte x: {} y: {} z: {} fps: {}",
                camera.position.x,
                camera.position.y,
                camera.position.z,
                1. / dt
            ));
            //todo: send less frequently
            if first_teleport {
                connection.send_message(&NetworkMessageC2S::PlayerPosition(
                    camera.position.x,
                    camera.position.y,
                    camera.position.z,
                    camera.is_shifting(),
                    camera.pitch_deg,
                    camera.last_moved,
                ));
            }
            for message in connection.read_messages() {
                match message {
                    NetworkMessageS2C::SetBlock(_, _, _, _) => {}
                    NetworkMessageS2C::LoadChunk(x, y, z, palette, blocks) => {
                        let position = ChunkPosition { x, y, z };
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
                    NetworkMessageS2C::UnloadChunk(x, y, z) => {
                        world.unload_chunk(ChunkPosition { x, y, z });
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
                            .unwrap();
                        render_state.window().set_cursor_visible(!locked);
                        render_state
                            .window()
                            .set_cursor_position(PhysicalPosition {
                                x: render_state.size().width as f32 / 2.,
                                y: render_state.size().height as f32 / 2.,
                            })
                            .unwrap();
                    }
                    NetworkMessageS2C::AddEntity(_, _, _, _, _, _, _, _) => {}
                    NetworkMessageS2C::MoveEntity(_, _, _, _, _) => {}
                    NetworkMessageS2C::DeleteEntity(_) => {}
                    NetworkMessageS2C::BlockBreakTimeResponse(_, _) => {}
                    NetworkMessageS2C::EntityItem(_, _, _) => {}
                    NetworkMessageS2C::BlockItem(_, _, _, _, _) => {}
                    NetworkMessageS2C::Knockback(_, _, _, _) => {}
                    NetworkMessageS2C::FluidSelectable(_) => {}
                    NetworkMessageS2C::PlaySound(_, _, _, _, _, _, _) => {}
                    NetworkMessageS2C::EntityAnimation(_, _) => {}
                    NetworkMessageS2C::ChatMessage(_) => {}
                    NetworkMessageS2C::PlayerAbilities(_, _) => {}
                    NetworkMessageS2C::TeleportPlayer(x, y, z, rotation) => {
                        camera.position = Point3::new(x, y, z);
                        camera.pitch_deg = rotation;
                        first_teleport = true;
                    }
                    NetworkMessageS2C::BlockAnimation(_, _, _, _) => {}
                }
            }
            match render_state.render(&camera, &mut world, &mut gui, &item_registry) {
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
