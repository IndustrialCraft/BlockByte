#![feature(fn_traits)]

mod content;
mod game;
mod render;
mod texture;

use block_byte_common::{ChunkPosition, Position};
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
    world.load_chunk(ChunkPosition { x: 0, y: 0, z: 0 }, [[[0u32; 16]; 16]; 16]);
    world.load_chunk(ChunkPosition { x: 0, y: 1, z: 0 }, [[[0u32; 16]; 16]; 16]);
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
