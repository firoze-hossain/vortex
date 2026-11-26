//! vortex-ui/src/draw.rs
//! High-performance 2D software drawing primitives and scaled thumbnail blitter for 32-bit ARGB surfaces.

use crate::geom::Rect;
use crate::image_decode::DecodedImage;

pub fn fill_rect(surface: &mut [u32], surf_w: usize, surf_h: usize, rect: Rect, color: u32) {
    if surface.is_empty() || surf_w == 0 || surf_h == 0 {
        return;
    }
    let x0 = rect.x.clamp(0, surf_w as i32) as usize;
    let y0 = rect.y.clamp(0, surf_h as i32) as usize;
    let x1 = rect.right().clamp(0, surf_w as i32) as usize;
    let y1 = rect.bottom().clamp(0, surf_h as i32) as usize;

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let alpha = (color >> 24) & 0xFF;
    if alpha == 0 {
        return;
    }

    if alpha == 255 {
        for y in y0..y1 {
            let row_start = y * surf_w + x0;
            let row_end = y * surf_w + x1;
            surface[row_start..row_end].fill(color);
        }
    } else {
        for y in y0..y1 {
            for x in x0..x1 {
                let idx = y * surf_w + x;
                surface[idx] = blend_pixel(surface[idx], color);
            }
        }
    }
}

pub fn draw_rect_outline(surface: &mut [u32], surf_w: usize, surf_h: usize, rect: Rect, thickness: i32, color: u32) {
    if rect.w <= 0 || rect.h <= 0 {
        return;
    }
    let t = thickness.max(1);
    fill_rect(surface, surf_w, surf_h, Rect::new(rect.x, rect.y, rect.w, t), color);
    fill_rect(surface, surf_w, surf_h, Rect::new(rect.x, rect.bottom() - t, rect.w, t), color);
    fill_rect(surface, surf_w, surf_h, Rect::new(rect.x, rect.y, t, rect.h), color);
    fill_rect(surface, surf_w, surf_h, Rect::new(rect.right() - t, rect.y, t, rect.h), color);
}

pub fn fill_rounded_rect(surface: &mut [u32], surf_w: usize, surf_h: usize, rect: Rect, radius: i32, color: u32) {
    if rect.w <= 0 || rect.h <= 0 {
        return;
    }
    let r = radius.clamp(0, (rect.w / 2).min(rect.h / 2));
    if r <= 1 {
        fill_rect(surface, surf_w, surf_h, rect, color);
        return;
    }

    fill_rect(surface, surf_w, surf_h, Rect::new(rect.x + r, rect.y, rect.w - r * 2, rect.h), color);
    fill_rect(surface, surf_w, surf_h, Rect::new(rect.x, rect.y + r, r, rect.h - r * 2), color);
    fill_rect(surface, surf_w, surf_h, Rect::new(rect.right() - r, rect.y + r, r, rect.h - r * 2), color);

    fill_corner_circle(surface, surf_w, surf_h, rect.x + r, rect.y + r, r, true, true, color);
    fill_corner_circle(surface, surf_w, surf_h, rect.right() - r, rect.y + r, r, false, true, color);
    fill_corner_circle(surface, surf_w, surf_h, rect.x + r, rect.bottom() - r, r, true, false, color);
    fill_corner_circle(surface, surf_w, surf_h, rect.right() - r, rect.bottom() - r, r, false, false, color);
}

pub fn draw_line(surface: &mut [u32], surf_w: usize, surf_h: usize, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) {
    let mut x = x0;
    let mut y = y0;
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        if x >= 0 && x < surf_w as i32 && y >= 0 && y < surf_h as i32 {
            let idx = (y as usize) * surf_w + (x as usize);
            if idx < surface.len() {
                surface[idx] = blend_pixel(surface[idx], color);
            }
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

pub fn draw_progress_bar(
    surface: &mut [u32],
    surf_w: usize,
    surf_h: usize,
    rect: Rect,
    pct: f32,
    bg_color: u32,
    fill_color: u32,
) {
    fill_rounded_rect(surface, surf_w, surf_h, rect, 3, bg_color);
    let fill_w = ((rect.w as f32) * (pct.clamp(0.0, 100.0) / 100.0)) as i32;
    if fill_w > 0 {
        let fill_rect = Rect::new(rect.x, rect.y, fill_w, rect.h);
        fill_rounded_rect(surface, surf_w, surf_h, fill_rect, 3, fill_color);
    }
    draw_rect_outline(surface, surf_w, surf_h, rect, 1, 0xFF3E4C5E);
}

/// Renders a decoded thumbnail image scaled smoothly to fit inside `dest` (preserving aspect ratio).
pub fn blit_image_fit(surface: &mut [u32], surf_w: usize, surf_h: usize, dest: Rect, img: &DecodedImage) {
    if dest.w <= 0 || dest.h <= 0 || img.width == 0 || img.height == 0 {
        return;
    }
    let scale_x = dest.w as f32 / img.width as f32;
    let scale_y = dest.h as f32 / img.height as f32;
    let scale = scale_x.min(scale_y);
    let draw_w = ((img.width as f32) * scale).round().max(1.0) as i32;
    let draw_h = ((img.height as f32) * scale).round().max(1.0) as i32;
    let ox = dest.x + (dest.w - draw_w) / 2;
    let oy = dest.y + (dest.h - draw_h) / 2;

    for dy in 0..draw_h {
        let py = oy + dy;
        if py < 0 || py >= surf_h as i32 {
            continue;
        }
        let sy = ((dy as usize) * img.height / (draw_h as usize)).min(img.height - 1);
        for dx in 0..draw_w {
            let px = ox + dx;
            if px < 0 || px >= surf_w as i32 {
                continue;
            }
            let sx = ((dx as usize) * img.width / (draw_w as usize)).min(img.width - 1);
            let col = img.pixels[sy * img.width + sx];
            let idx = (py as usize) * surf_w + (px as usize);
            if idx < surface.len() {
                surface[idx] = blend_pixel(surface[idx], col);
            }
        }
    }
}

/// Draws a simple crisp downward arrow icon at (x, y) within 10x10.
pub fn draw_down_arrow(surface: &mut [u32], surf_w: usize, surf_h: usize, x: i32, y: i32, color: u32) {
    fill_rect(surface, surf_w, surf_h, Rect::new(x + 4, y + 1, 2, 6), color);
    draw_line(surface, surf_w, surf_h, x + 2, y + 5, x + 5, y + 8, color);
    draw_line(surface, surf_w, surf_h, x + 7, y + 5, x + 4, y + 8, color);
}

/// Draws a simple crisp upward arrow icon at (x, y) within 10x10.
pub fn draw_up_arrow(surface: &mut [u32], surf_w: usize, surf_h: usize, x: i32, y: i32, color: u32) {
    fill_rect(surface, surf_w, surf_h, Rect::new(x + 4, y + 3, 2, 6), color);
    draw_line(surface, surf_w, surf_h, x + 2, y + 4, x + 5, y + 1, color);
    draw_line(surface, surf_w, surf_h, x + 7, y + 4, x + 4, y + 1, color);
}

fn fill_corner_circle(
    surface: &mut [u32],
    surf_w: usize,
    surf_h: usize,
    cx: i32,
    cy: i32,
    r: i32,
    left: bool,
    top: bool,
    color: u32,
) {
    for dy in 0..=r {
        for dx in 0..=r {
            if dx * dx + dy * dy <= r * r {
                let px = if left { cx - dx } else { cx + dx };
                let py = if top { cy - dy } else { cy + dy };
                if px >= 0 && px < surf_w as i32 && py >= 0 && py < surf_h as i32 {
                    let idx = (py as usize) * surf_w + (px as usize);
                    if idx < surface.len() {
                        surface[idx] = blend_pixel(surface[idx], color);
                    }
                }
            }
        }
    }
}

pub fn blend_pixel(bg: u32, fg: u32) -> u32 {
    let a = (fg >> 24) & 0xFF;
    if a == 255 {
        return fg;
    }
    if a == 0 {
        return bg;
    }

    let inv_a = 255 - a;
    let r = (((fg >> 16) & 0xFF) * a + ((bg >> 16) & 0xFF) * inv_a) / 255;
    let g = (((fg >> 8) & 0xFF) * a + ((bg >> 8) & 0xFF) * inv_a) / 255;
    let b = ((fg & 0xFF) * a + (bg & 0xFF) * inv_a) / 255;

    0xFF000000 | (r << 16) | (g << 8) | b
}
