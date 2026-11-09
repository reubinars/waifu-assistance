//! Platform-independent logic for the island: state, layout, animation maths, herdr protocol.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub mod anim;
pub mod agents;
pub mod color;
pub mod herdr;
pub mod media;
pub mod session;
pub mod view;
