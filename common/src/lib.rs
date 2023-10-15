#![feature(int_roundings)]

pub mod block_palette;
pub mod content;
pub mod gui;
pub mod messages;

use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::ops;
use std::ops::Neg;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HorizontalFace {
    Front,
    Back,
    Left,
    Right,
}
impl HorizontalFace {
    const HORIZONTAL_FACES: [HorizontalFace; 4] = [
        HorizontalFace::Front,
        HorizontalFace::Back,
        HorizontalFace::Left,
        HorizontalFace::Right,
    ];
    pub fn all() -> &'static [HorizontalFace; 4] {
        &HorizontalFace::HORIZONTAL_FACES
    }
    pub fn to_face(&self) -> Face {
        match self {
            HorizontalFace::Front => Face::Front,
            HorizontalFace::Back => Face::Back,
            HorizontalFace::Left => Face::Left,
            HorizontalFace::Right => Face::Right,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Face {
    Front,
    Back,
    Up,
    Down,
    Left,
    Right,
}
impl Face {
    const FACES: [Face; 6] = [
        Face::Front,
        Face::Back,
        Face::Up,
        Face::Down,
        Face::Left,
        Face::Right,
    ];
    pub fn all() -> &'static [Face; 6] {
        &Face::FACES
    }
    #[inline(always)]
    pub fn get_offset(&self) -> BlockPosition {
        match self {
            Self::Front => BlockPosition { x: 0, y: 0, z: -1 },
            Self::Back => BlockPosition { x: 0, y: 0, z: 1 },
            Self::Left => BlockPosition { x: -1, y: 0, z: 0 },
            Self::Right => BlockPosition { x: 1, y: 0, z: 0 },
            Self::Up => BlockPosition { x: 0, y: 1, z: 0 },
            Self::Down => BlockPosition { x: 0, y: -1, z: 0 },
        }
    }
    #[inline(always)]
    pub fn opposite(&self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
            Self::Front => Self::Back,
            Self::Back => Self::Front,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
    pub fn to_horizontal_face(&self) -> Option<HorizontalFace> {
        match self {
            Face::Front => Some(HorizontalFace::Front),
            Face::Back => Some(HorizontalFace::Back),
            Face::Left => Some(HorizontalFace::Left),
            Face::Right => Some(HorizontalFace::Right),
            _ => None,
        }
    }
}
pub struct FaceStorage<T> {
    pub front: T,
    pub back: T,
    pub left: T,
    pub right: T,
    pub up: T,
    pub down: T,
}
impl<T> FaceStorage<T> {
    pub fn by_face(&self, face: Face) -> &T {
        match face {
            Face::Front => &self.front,
            Face::Back => &self.back,
            Face::Left => &self.left,
            Face::Right => &self.right,
            Face::Up => &self.up,
            Face::Down => &self.down,
        }
    }
    pub fn by_face_mut(&mut self, face: Face) -> &mut T {
        match face {
            Face::Front => &mut self.front,
            Face::Back => &mut self.back,
            Face::Left => &mut self.left,
            Face::Right => &mut self.right,
            Face::Up => &mut self.up,
            Face::Down => &mut self.down,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}
impl Position {
    pub fn add(&self, x: f64, y: f64, z: f64) -> Self {
        Self {
            x: self.x + x,
            y: self.y + y,
            z: self.z + z,
        }
    }
    pub fn multiply(&self, scalar: f64) -> Self {
        Position {
            x: self.x * scalar,
            y: self.y * scalar,
            z: self.z * scalar,
        }
    }
    pub fn distance(&self, other: &Position) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2) + (self.z - other.z).powi(2))
            .sqrt()
    }
    pub fn get_x(&mut self) -> f64 {
        self.x
    }
    pub fn set_x(&mut self, value: f64) {
        self.x = value;
    }
    pub fn get_y(&mut self) -> f64 {
        self.y
    }
    pub fn set_y(&mut self, value: f64) {
        self.y = value;
    }
    pub fn get_z(&mut self) -> f64 {
        self.z
    }
    pub fn set_z(&mut self, value: f64) {
        self.z = value;
    }
}
impl std::ops::Add for Position {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Position {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BlockPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}
impl Neg for BlockPosition {
    type Output = BlockPosition;
    fn neg(self) -> Self::Output {
        BlockPosition {
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
}
impl Display for BlockPosition {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "({},{},{})", self.x, self.y, self.z)
    }
}
impl BlockPosition {
    pub fn offset_by_face(&self, face: Face) -> BlockPosition {
        *self + face.get_offset()
    }
}
impl std::ops::Add for BlockPosition {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        BlockPosition {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}
impl BlockPosition {
    #[inline(always)]
    pub fn offset_from_origin_chunk(&self) -> Option<Face> {
        if self.x < 0 {
            return Some(Face::Left);
        }
        if self.x >= 16 {
            return Some(Face::Right);
        }
        if self.y < 0 {
            return Some(Face::Down);
        }
        if self.y >= 16 {
            return Some(Face::Up);
        }
        if self.z < 0 {
            return Some(Face::Front);
        }
        if self.z >= 16 {
            return Some(Face::Back);
        }
        return None;
    }
    #[inline(always)]
    pub fn chunk_offset(&self) -> (u8, u8, u8) {
        (
            self.x.rem_euclid(16) as u8,
            self.y.rem_euclid(16) as u8,
            self.z.rem_euclid(16) as u8,
        )
    }
    #[inline(always)]
    pub fn to_chunk_pos(&self) -> ChunkPosition {
        ChunkPosition {
            x: self.x.div_floor(16),
            y: self.y.div_floor(16),
            z: self.z.div_floor(16),
        }
    }
    #[inline(always)]
    pub fn to_position(&self) -> Position {
        Position {
            x: self.x as f64,
            y: self.y as f64,
            z: self.z as f64,
        }
    }
}
impl Position {
    #[inline(always)]
    pub fn to_chunk_pos(&self) -> ChunkPosition {
        ChunkPosition {
            x: ((self.x as f32) / 16f32).floor() as i32,
            y: ((self.y as f32) / 16f32).floor() as i32,
            z: ((self.z as f32) / 16f32).floor() as i32,
        }
    }
    #[inline(always)]
    pub fn to_block_pos(&self) -> BlockPosition {
        BlockPosition {
            x: self.x.floor() as i32,
            y: self.y.floor() as i32,
            z: self.z.floor() as i32,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct ChunkPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}
impl ChunkPosition {
    pub fn with_offset(&self, face: &Face) -> Self {
        let offset = face.get_offset();
        ChunkPosition {
            x: self.x + offset.x as i32,
            y: self.y + offset.y as i32,
            z: self.z + offset.z as i32,
        }
    }
    pub fn add(&self, x: i32, y: i32, z: i32) -> Self {
        ChunkPosition {
            x: self.x + x,
            y: self.y + y,
            z: self.z + z,
        }
    }
    pub fn distance_squared(&self, other: &ChunkPosition) -> u32 {
        let xd = self.x - other.x;
        let yd = self.y - other.y;
        let zd = self.z - other.z;
        (xd * xd + yd * yd + zd * zd) as u32
    }
}
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub const ZERO: Vec3 = Vec3 {
        x: 0.,
        y: 0.,
        z: 0.,
    };
    pub const ONE: Vec3 = Vec3 {
        x: 1.,
        y: 1.,
        z: 1.,
    };
}
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}
impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0., y: 0. };
}
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct TexCoords {
    pub u1: f32,
    pub v1: f32,
    pub u2: f32,
    pub v2: f32,
}
impl TexCoords {
    pub fn map_sub(&self, sub: &TexCoords) -> TexCoords {
        let self_w = self.u2 - self.u1;
        let self_h = self.v2 - self.v1;
        TexCoords {
            u1: self.u1 + (sub.u1 * self_w),
            v1: self.v1 + (sub.v1 * self_h),
            u2: self.u1 + (sub.u2 * self_w),
            v2: self.v1 + (sub.v2 * self_h),
        }
    }
    pub fn flip_horizontally(&self) -> TexCoords {
        TexCoords {
            u1: self.u2,
            v1: self.v1,
            u2: self.u1,
            v2: self.v2,
        }
    }
}
#[derive(Copy, Clone, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}
impl Color {
    pub const WHITE: Color = Color {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    pub fn from_array(data: [u8; 4]) -> Color {
        Color {
            r: data[0],
            g: data[1],
            b: data[2],
            a: data[3],
        }
    }
    pub fn to_array(&self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }
}
impl ops::Mul for Color {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        //todo: copied from https://stackoverflow.com/questions/45041273/how-to-correctly-multiply-two-colors-with-byte-components, check if works
        Color {
            r: ((self.r as u16 * rhs.r as u16 + 0xFF) >> 8) as u8,
            g: ((self.g as u16 * rhs.g as u16 + 0xFF) >> 8) as u8,
            b: ((self.b as u16 * rhs.b as u16 + 0xFF) >> 8) as u8,
            a: ((self.a as u16 * rhs.a as u16 + 0xFF) >> 8) as u8,
        }
    }
}

pub struct AABB {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
    pub h: f64,
    pub d: f64,
}
impl AABB {
    pub fn raycast(&self, position: Position, direction: Vec3) -> Option<f64> {
        let t1 = (self.x - position.x) / direction.x as f64;
        let t2 = ((self.x + self.w) - position.x) / direction.x as f64;
        let t3 = (self.y - position.y) / direction.y as f64;
        let t4 = ((self.y + self.h) - position.y) / direction.y as f64;
        let t5 = (self.z - position.z) / direction.z as f64;
        let t6 = ((self.z + self.d) - position.z) / direction.z as f64;

        let tmin = ((t1.min(t2)).max(t3.min(t4))).max(t5.min(t6));
        let tmax = ((t1.max(t2)).min(t3.max(t4))).min(t5.max(t6));

        if tmax < 0. {
            return None;
        }
        if tmin > tmax {
            return None;
        }
        if tmin < 0. {
            return Some(tmax);
        }
        return Some(tmin);
    }
    pub fn calc_second_point(&self) -> (f64, f64, f64) {
        (self.x + self.w, self.y + self.h, self.z + self.d)
    }
    pub fn collides(&self, other: &AABB) -> bool {
        let (x2, y2, z2) = self.calc_second_point();
        let (other_x2, other_y2, other_z2) = other.calc_second_point();

        x2 > other.x
            && self.x < other_x2
            && y2 > other.y
            && self.y < other_y2
            && z2 > other.z
            && self.z < other_z2
    }
    pub fn move_by(&self, x: f64, y: f64, z: f64) -> AABB {
        AABB {
            x: self.x + x,
            y: self.y + y,
            z: self.z + z,
            w: self.w,
            h: self.h,
            d: self.d,
        }
    }
    pub fn set_position(&mut self, position: Position) {
        self.x = position.x;
        self.y = position.y;
        self.z = position.z;
    }
    pub fn get_position(&mut self) -> Position {
        Position {
            x: self.x,
            y: self.y,
            z: self.z,
        }
    }
    pub fn iter_blocks(&self) -> AABBBlockIterator {
        let second_point = self.calc_second_point();
        let iterator = AABBBlockIterator {
            start_x: (self.x + 0.05).floor() as i32,
            start_y: (self.y + 0.05).floor() as i32,
            end_x: (second_point.0 - 0.05).ceil() as i32 - 1,
            end_y: (second_point.1 - 0.05).ceil() as i32 - 1,
            end_z: (second_point.2 - 0.05).ceil() as i32 - 1,
            x: (self.x + 0.05).floor() as i32,
            y: (self.y + 0.05).floor() as i32,
            z: (self.z + 0.05).floor() as i32,
            finished: false,
        };
        iterator
    }
}
pub struct AABBBlockIterator {
    start_x: i32,
    start_y: i32,
    end_x: i32,
    end_y: i32,
    end_z: i32,
    x: i32,
    y: i32,
    z: i32,
    finished: bool,
}

impl Iterator for AABBBlockIterator {
    type Item = BlockPosition;
    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        if self.x > self.end_x {
            self.x = self.start_x;
            self.y += 1;
            if self.y > self.end_y {
                self.y = self.start_y;
                self.z += 1;
                if self.z > self.end_z {
                    self.finished = true;
                    return None;
                }
            }
        }
        let return_position = Some(BlockPosition {
            x: self.x,
            y: self.y,
            z: self.z,
        });
        self.x += 1;
        return_position
    }
}
#[allow(non_snake_case)]
pub mod KeyboardModifier {
    pub const SHIFT: u8 = 1;
    pub const CTRL: u8 = 2;
    pub const ALT: u8 = 4;
}

//from winit
#[derive(Serialize, Deserialize)]
pub enum KeyboardKey {
    Key1,
    Key2,
    Key3,
    Key4,
    Key5,
    Key6,
    Key7,
    Key8,
    Key9,
    Key0,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Escape,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
    Snapshot,
    Scroll,
    Pause,
    Insert,
    Home,
    Delete,
    End,
    PageDown,
    PageUp,
    Left,
    Up,
    Right,
    Down,
    Backspace,
    Enter,
    Space,
    Compose,
    Caret,
    Numlock,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadDivide,
    NumpadDecimal,
    NumpadComma,
    NumpadEnter,
    NumpadEquals,
    NumpadMultiply,
    NumpadSubtract,
    AbntC1,
    AbntC2,
    Apostrophe,
    Apps,
    Asterisk,
    At,
    Ax,
    Backslash,
    Calculator,
    Capital,
    Colon,
    Comma,
    Convert,
    Equals,
    Grave,
    Kana,
    Kanji,
    LAlt,
    LBracket,
    LControl,
    LShift,
    LWin,
    Mail,
    MediaSelect,
    MediaStop,
    Minus,
    Mute,
    MyComputer,
    NavigateForward,
    NavigateBackward,
    NextTrack,
    NoConvert,
    OEM102,
    Period,
    PlayPause,
    Plus,
    Power,
    PrevTrack,
    RAlt,
    RBracket,
    RControl,
    RShift,
    RWin,
    Semicolon,
    Slash,
    Sleep,
    Stop,
    Sysrq,
    Tab,
    Underline,
    Unlabeled,
    VolumeDown,
    VolumeUp,
    Wake,
    WebBack,
    WebFavorites,
    WebForward,
    WebHome,
    WebRefresh,
    WebSearch,
    WebStop,
    Yen,
    Copy,
    Paste,
    Cut,
}
impl KeyboardKey {
    pub fn get_slot(&self) -> Option<u8> {
        match self {
            KeyboardKey::Key1 => Some(0),
            KeyboardKey::Key2 => Some(1),
            KeyboardKey::Key3 => Some(2),
            KeyboardKey::Key4 => Some(3),
            KeyboardKey::Key5 => Some(4),
            KeyboardKey::Key6 => Some(5),
            KeyboardKey::Key7 => Some(6),
            KeyboardKey::Key8 => Some(7),
            KeyboardKey::Key9 => Some(8),
            KeyboardKey::Key0 => Some(9),
            _ => None,
        }
    }
}
