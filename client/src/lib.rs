mod game;
mod render;
mod texture;

use block_byte_common::Position;
use std::collections::HashSet;
use winit::window::CursorGrabMode;
use winit::{
    event::*,
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

use crate::game::ClientPlayer;
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

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new().build(&event_loop).unwrap();
    window.set_cursor_grab(CursorGrabMode::Confined).unwrap();
    window.set_cursor_visible(false);
    #[cfg(target_arch = "wasm32")]
    {
        // Winit prevents sizing with CSS, so we have to set
        // the size manually when on web.
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
    let mut render_state = RenderState::new(window).await;
    let mut camera = ClientPlayer::at_position(Position {
        x: 0.,
        y: 0.,
        z: 0.,
    });
    let mut keys = HashSet::new();
    event_loop.run(move |event, _, control_flow| match event {
        Event::WindowEvent {
            ref event,
            window_id,
        } if window_id == render_state.window().id() => {
            match event {
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
                    // new_inner_size is &&mut so w have to dereference it twice
                    render_state.resize(**new_inner_size);
                }
                _ => {}
            }
        }
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
            camera.update_position(&keys, 1. / 60.);
            render_state.window().set_title(&format!(
                "BlockByte x: {} y: {} z: {}",
                camera.position.x, camera.position.y, camera.position.z
            ));
            match render_state.render(&camera) {
                Ok(_) => {}
                // Reconfigure the surface if it's lost or outdated
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    render_state.resize(render_state.size())
                }
                // The system is out of memory, we should probably quit
                Err(wgpu::SurfaceError::OutOfMemory) => *control_flow = ControlFlow::Exit,

                Err(wgpu::SurfaceError::Timeout) => log::warn!("Surface timeout"),
            }
        }
        Event::RedrawEventsCleared => {
            // RedrawRequested will only trigger once, unless we manually
            // request it.
            render_state.window().request_redraw();
        }
        _ => {}
    })
}
