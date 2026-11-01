use gtk4::gdk;
use std::path::Path;

/// A hand-on-a-stick image plus the landmarks needed to aim it.
///
/// Rows are fractions of the image height, columns fractions of its width.
/// The measurement matches TinyHand's `HandAsset.measure`: the hand points up,
/// ends in a wrist or cuff, and continues down into a rod.
pub struct Hand {
    pub texture: gdk::Texture,
    pub width: f32,
    pub height: f32,
    pub hand_top: f32,
    pub tip: f32,
    pub wrist: f32,
    pub rod_end: f32,
    pub tip_x: f32,
    pub rod_x: f32,
}

impl Hand {
    pub fn load(path: &Path) -> Result<Hand, String> {
        let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let pixbuf = gdk_pixbuf::Pixbuf::from_file(path).map_err(|e| fail(&e))?;
        if !pixbuf.has_alpha() || pixbuf.n_channels() != 4 {
            return Err(fail(&"hand image needs an alpha channel"));
        }
        let w = pixbuf.width() as usize;
        let h = pixbuf.height() as usize;
        let stride = pixbuf.rowstride() as usize;
        let bytes = pixbuf.read_pixel_bytes();
        let opaque = |x: usize, y: usize| bytes[y * stride + x * 4 + 3] > 128;

        let counts: Vec<usize> = (0..h).map(|y| (0..w).filter(|&x| opaque(x, y)).count()).collect();
        let center = |y: usize| {
            let mut xs = (0..w).filter(|&x| opaque(x, y));
            match xs.next() {
                Some(lo) => (lo + xs.next_back().unwrap_or(lo)) as f32 / 2.0 / w as f32,
                None => 0.5,
            }
        };

        let shape = || fail(&"could not find a hand on a rod in the image");
        let top = counts.iter().position(|&c| c > 0).ok_or_else(shape)?;
        let search_end = h.min(top + (0.45 * h as f64) as usize);
        let max_w = counts[top..search_end].iter().copied().max().unwrap_or(0) as f64;
        let fist = (top..h).find(|&y| counts[y] as f64 > 0.6 * max_w).ok_or_else(shape)?;
        let wrist = (fist..h).find(|&y| (counts[y] as f64) < 0.35 * max_w).ok_or_else(shape)?;
        let rod_w = counts[(h - 1).min(wrist + h / 50)] as f64;
        let last_row = counts.iter().rposition(|&c| c > 0).unwrap_or(h - 1);
        let from = (h - 1).min(wrist + h / 33);
        let rod_end = (from..h).find(|&y| counts[y] as f64 > 1.8 * rod_w).unwrap_or(last_row);

        let fh = h as f32;
        Ok(Hand {
            texture: gdk::Texture::from_filename(path).map_err(|e| fail(&e))?,
            width: w as f32,
            height: fh,
            hand_top: (top as f32 - 0.004 * fh).max(0.0) / fh,
            tip: top as f32 / fh,
            wrist: wrist as f32 / fh,
            rod_end: rod_end as f32 / fh,
            tip_x: center((h - 1).min(top + h / 200)),
            rod_x: center((wrist + rod_end) / 2),
        })
    }
}
