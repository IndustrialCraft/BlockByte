use block_byte_common::Position;
use cgmath::{ElementWise, InnerSpace, Matrix4, Point3, Vector3};
use winit::event::VirtualKeyCode;

pub struct ClientPlayer {
    pub position: Point3<f32>,
    pub velocity: Vector3<f32>,
    pub pitch_deg: f32,
    pub yaw_deg: f32,
    shifting: bool,
    shifting_animation: f32,
    pub last_moved: bool,
    pub speed: f32,
}
impl ClientPlayer {
    const UP: Vector3<f32> = Vector3 {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    pub fn is_shifting(&self) -> bool {
        self.shifting
    }
    pub fn make_front(&self) -> Vector3<f32> {
        let pitch_rad = f32::to_radians(self.pitch_deg);
        let yaw_rad = f32::to_radians(self.yaw_deg);
        Vector3 {
            x: yaw_rad.sin() * pitch_rad.cos(),
            y: pitch_rad.sin(),
            z: yaw_rad.cos() * pitch_rad.cos(),
        }
    }
    pub fn update_orientation(&mut self, d_pitch_deg: f32, d_yaw_deg: f32) {
        self.pitch_deg = (self.pitch_deg + d_pitch_deg).max(-89.0).min(89.0);
        self.yaw_deg = (self.yaw_deg + d_yaw_deg) % 360.0;
    }
    pub fn knockback(&mut self, x: f32, y: f32, z: f32, set: bool) {
        if set {
            self.velocity = Vector3::new(0., 0., 0.);
        }
        self.velocity += Vector3::new(x, y, z);
    }
    pub fn update_position(
        &mut self,
        keys: &std::collections::HashSet<VirtualKeyCode>,
        delta_time: f32,
    ) {
        let mut forward = self.make_front();
        forward.y = 0.;
        let cross_normalized = forward.cross(Self::UP).normalize();
        let mut move_vector = keys.iter().copied().fold(
            Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            |vec, key| match key {
                VirtualKeyCode::W => vec + forward,
                VirtualKeyCode::S => vec - forward,
                VirtualKeyCode::A => vec - cross_normalized,
                VirtualKeyCode::D => vec + cross_normalized,
                _ => vec,
            },
        );
        self.shifting = keys.contains(&VirtualKeyCode::LShift);

        if !(move_vector.x == 0.0 && move_vector.y == 0.0 && move_vector.z == 0.0) {
            move_vector = move_vector.normalize();
        }
        if self.shifting {
            move_vector.y -= 1.;
            move_vector /= 2.;
        }
        if keys.contains(&VirtualKeyCode::Space) {
            move_vector.y += 1.;
        }

        move_vector *= self.speed;
        move_vector *= 5.;

        let mut total_move = (move_vector + self.velocity) * delta_time;

        self.last_moved = move_vector.magnitude() > 0.;

        let drag_coefficient = 0.025;
        let drag = self
            .velocity
            .mul_element_wise(self.velocity)
            .mul_element_wise(Vector3 {
                x: 1f32.copysign(self.velocity.x),
                y: 1f32.copysign(self.velocity.y),
                z: 1f32.copysign(self.velocity.z),
            })
            * drag_coefficient;
        self.velocity -= drag * delta_time;
        self.position += total_move;
        //self.velocity.y -= delta_time * 15f32;

        self.shifting_animation += (if self.shifting { 1. } else { -1. }) * delta_time * 4.;
        self.shifting_animation = self.shifting_animation.clamp(0., 0.5);
    }
    pub const fn at_position(position: Position) -> Self {
        Self {
            position: Point3 {
                x: position.x as f32,
                y: position.y as f32,
                z: position.z as f32,
            },
            velocity: Vector3::new(0., 0., 0.),
            pitch_deg: 0.0,
            yaw_deg: 0.0,
            shifting: false,
            shifting_animation: 0f32,
            last_moved: false,
            speed: 1.,
        }
    }
    fn eye_height_diff(&self) -> f32 {
        1.75 - self.shifting_animation
    }
    pub fn get_eye(&self) -> Position {
        Position {
            x: self.position.x as f64,
            y: self.position.y as f64,
            z: self.position.z as f64,
        }
        .add(0., self.eye_height_diff() as f64, 0.)
    }
    pub fn create_view_matrix(&self) -> Matrix4<f32> {
        Matrix4::look_at_rh(
            self.position
                + Vector3 {
                    x: 0.,
                    y: self.eye_height_diff(),
                    z: 0.,
                },
            self.position
                + Vector3 {
                    x: 0.,
                    y: self.eye_height_diff(),
                    z: 0.,
                }
                + self.make_front(),
            Self::UP,
        )
    }
    pub fn create_view_matrix_no_pos(&self) -> Matrix4<f32> {
        Matrix4::look_at_rh(
            Point3 {
                x: 0.,
                y: 0.,
                z: 0.,
            },
            Point3 {
                x: 0.,
                y: 0.,
                z: 0.,
            } + self.make_front(),
            Self::UP,
        )
    }
    pub fn create_projection_matrix(&self, aspect: f32) -> Matrix4<f32> {
        cgmath::perspective(cgmath::Deg(90.), aspect, 0.1, 100.)
    }
}
