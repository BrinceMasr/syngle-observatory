//! Which graphics context the page's canvas gets, and what it is asked for.
//!
//! The map paints every pixel of its canvas and draws nothing in 3D, so when the browser composites it through WebGL2 it
//! asks for a context with no alpha channel (the page's background is never blended through the canvas), no depth plane and
//! no stencil plane (nothing to allocate or clear each frame). On a software rasterizer, which pays for a frame by the
//! pixels it fills, that is the difference between a frame in one 60 Hz slot and one in two. A canvas keeps the first
//! context made on it and answers a later `getContext` of the same type with that one, so the options have to be given
//! before wgpu asks for its own, and only when wgpu will ask for WebGL2: a canvas that has a WebGL2 context cannot take
//! a WebGPU one.

/// The context attributes the map's canvas turns off.
pub const OFF: [&str; 4] = ["alpha", "depth", "stencil", "antialias"];

/// Whether wgpu will draw this canvas through WebGL2: the page asked for it (`?webgl`, `?backend=webgl2`), or the browser
/// has no WebGPU for it to use.
pub fn webgl2_is_the_backend(query: &str, browser_has_webgpu: bool) -> bool {
    query.contains("webgl") || !browser_has_webgpu
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_context_is_made_only_when_wgpu_will_draw_through_webgl2() {
        // Asked for by the page, WebGPU or not.
        assert!(webgl2_is_the_backend("?project=1&backend=webgl2", true));
        assert!(webgl2_is_the_backend("?webgl", false));
        // No WebGPU in the browser: wgpu falls back to WebGL2 by itself.
        assert!(webgl2_is_the_backend("", false));
        // WebGPU in the browser and nobody asked otherwise: the canvas must stay free for a WebGPU context.
        assert!(!webgl2_is_the_backend("", true));
        assert!(!webgl2_is_the_backend("?embed&project=1", true));
    }

    #[test]
    fn what_the_canvas_turns_off() {
        assert_eq!(OFF, ["alpha", "depth", "stencil", "antialias"]);
    }
}
