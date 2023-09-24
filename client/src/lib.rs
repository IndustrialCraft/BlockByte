#![feature(fn_traits)]
#![feature(map_many_mut)]

mod content;
mod game;
mod model;
mod net;
mod render;
mod texture;

use array_init::array_init;
use block_byte_common::messages::{NetworkMessageC2S, NetworkMessageS2C};
use block_byte_common::{ChunkPosition, Position};
use cgmath::Point3;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;
use winit::window::CursorGrabMode;
use winit::{
    event::*,
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

use crate::game::{ClientPlayer, World};
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
    let (texture_image, block_registry) =
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

    let mut connection = SocketConnection::new("localhost:4321");
    let mut first_teleport = false;
    let mut last_render_time = Instant::now();
    event_loop.run(move |event, _, control_flow| match event {
        Event::WindowEvent {
            ref event,
            window_id,
        } if window_id == render_state.window().id() => match event {
            WindowEvent::CloseRequested
            | WindowEvent::KeyboardInput {
                input:
                    KeyboardInput {
                        state: ElementState::Pressed,
                        virtual_keycode: Some(VirtualKeyCode::Escape),
                        ..
                    },
                ..
            } => *control_flow = ControlFlow::Exit,
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
                }
            }
            WindowEvent::Resized(physical_size) => {
                render_state.resize(*physical_size);
            }
            WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                render_state.resize(**new_inner_size);
            }
            _ => {}
        },
        Event::DeviceEvent {
            ref event,
            device_id: _,
        } => match event {
            DeviceEvent::MouseMotion { delta: (x, y) } => {
                camera.update_orientation(-*y as f32, -*x as f32);
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
                    NetworkMessageS2C::AddEntity(_, _, _, _, _, _, _, _) => {}
                    NetworkMessageS2C::MoveEntity(_, _, _, _, _) => {}
                    NetworkMessageS2C::DeleteEntity(_) => {}
                    NetworkMessageS2C::GuiData(_) => {}
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
            match render_state.render(&camera, &mut world) {
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
