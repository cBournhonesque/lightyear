pub(crate) fn ease_out_quad(x: f32) -> f32 {
    1.0 - (1.0 - x) * (1.0 - x)
}

pub(crate) fn ease_out_expo(x: f32) -> f32 {
    if x >= 0.99 {
        1.0
    } else {
        1.0 - 2.0_f32.powf(-6.0 * x)
    }
}

pub(crate) fn ease_out_cubic(x: f32) -> f32 {
    1.0 - (1.0 - x).powf(3.0)
}

pub(crate) fn ease_out_quart(x: f32) -> f32 {
    1.0 - (1.0 - x).powf(4.0)
}
