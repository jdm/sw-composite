pub mod blend;

const BILINEAR_INTERPOLATION_BITS: u32 = 4;

const A32_SHIFT: u32 = 24;
const R32_SHIFT: u32 = 16;
const G32_SHIFT: u32 = 8;
const B32_SHIFT: u32 = 0;


type Alpha256 = u32;

/// A unpremultiplied color
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Color {
    val: u32
}

impl Color {
    pub fn new(a: u8, r: u8, g: u8, b: u8) -> Color {
        Color { val:
        ((a as u32) << A32_SHIFT) |
        ((r as u32) << R32_SHIFT) |
        ((g as u32) << G32_SHIFT) |
        ((b as u32) << B32_SHIFT) }
    }
}

#[derive(Clone, Copy)]
pub struct Image<'a> {
    pub width: i32,
    pub height: i32,
    pub data: &'a [u32],
}

/// t is 0..256
#[inline]
pub fn lerp(a: u32, b: u32, t: u32) -> u32 {
    // we can reduce this to two multiplies
    // http://stereopsis.com/doubleblend.html
    let mask = 0xff00ff;
    let brb = ((b & 0xff00ff) * t) >> 8;
    let bag = ((b >> 8) & 0xff00ff) * t;
    let t = 256 - t;
    let arb = ((a & 0xff00ff) * t) >> 8;
    let aag = ((a >> 8) & 0xff00ff) * t;
    let rb = arb + brb;
    let ag = aag + bag;
    return (rb & mask) | (ag & !mask);
}

/// color is unpremultiplied argb
#[derive(Clone, Copy, Debug)]
pub struct GradientStop {
    pub position: f32,
    pub color: Color,
}

pub struct GradientSource {
    matrix: MatrixFixedPoint,
    lut: [u32; 256],
}

pub struct TwoCircleRadialGradientSource {
    matrix: MatrixFixedPoint,
    c1x: f32,
    c1y: f32,
    r1: f32,
    c2x: f32,
    c2y: f32,
    r2: f32,
    lut: [u32; 256],
}

#[derive(Clone, Copy)]
pub enum Spread {
    Pad,
    Reflect,
    Repeat,
}

fn apply_spread(mut x: i32, spread: Spread) -> i32 {
    match spread {
        Spread::Pad => {
            if x >= 255 {
                x = 255;
            }
            if x < 0 {
                x = 0;
            }
        }
        Spread::Repeat => {
            x &= 255;
        }
        Spread::Reflect => {
            // a trick from skia to reflect the bits. 256 -> 255
            let sign = (x << 23) >> 31;
            x = (x ^ sign) & 255;
        }
    }
    x
}

impl GradientSource {
    pub fn radial_gradient_eval(&self, x: u16, y: u16, spread: Spread) -> u32 {
        let p = self.matrix.transform(x, y);
        // there's no chance that p will overflow when squared
        // so it's safe to use sqrt
        let px = p.x as f32;
        let py = p.y as f32;
        let mut distance = (px * px + py * py).sqrt() as i32;
        distance >>= 8;

        self.lut[apply_spread(distance, spread) as usize]
    }

    pub fn linear_gradient_eval(&self, x: u16, y: u16, spread: Spread) -> u32 {
        let p = self.matrix.transform(x, y);
        let lx = p.x >> 8;

        self.lut[apply_spread(lx, spread) as usize]
    }
}

impl TwoCircleRadialGradientSource {
    pub fn eval(&self, x: u16, y: u16, spread: Spread) -> u32 {
        let p = self.matrix.transform(x, y);
        // XXX: this is slow and bad
        // the derivation is from pixman
        let px = p.x as f32 / 65536.;
        let py = p.y as f32 / 65536.;
        let cdx = self.c2x - self.c1x;
        let cdy = self.c2y - self.c1y;
        let pdx = px - self.c1x;
        let pdy = py - self.c1y;
        let dr = self.r2 - self.r1;
        let a = cdx*cdx + cdy*cdy - dr*dr;
        let b = pdx*cdx + pdy*cdy + self.r1*dr;
        let c = pdx*pdx + pdy*pdy - self.r1*self.r1;
        let t1 = (b + (b*b - a*c).sqrt())/a;
        let t2 = (b - (b*b - a*c).sqrt())/a;

        let t = if a == 0. {
            0.
        } else {
            if t1 > t2 {
                t1
            } else {
                t2
            }
        };

        self.lut[apply_spread((t * 255.) as i32, spread) as usize]
    }
}

#[derive(Clone, Debug)]
pub struct Gradient {
    pub stops: Vec<GradientStop>
}

impl Gradient {
    pub fn make_source(&self, matrix: &MatrixFixedPoint, alpha: u32) -> Box<GradientSource> {
        let mut source = Box::new(GradientSource { matrix: (*matrix).clone(), lut: [0; 256] });
        self.build_lut(&mut source.lut, alpha_to_alpha256(alpha));
        source
    }

    pub fn make_two_circle_source(&self, c1x: f32,
                                  c1y: f32,
                                  r1: f32,
                                  c2x: f32,
                                  c2y: f32,
                                  r2: f32, matrix: &MatrixFixedPoint, alpha: u32) -> Box<TwoCircleRadialGradientSource> {
        let mut source = Box::new(TwoCircleRadialGradientSource { c1x, c1y, r1, c2x, c2y, r2, matrix: (*matrix).clone(), lut: [0; 256] });
        self.build_lut(&mut source.lut, alpha_to_alpha256(alpha));
        source
    }

    fn build_lut(&self, lut: &mut [u32; 256], alpha: Alpha256) {
        let mut stop_idx = 0;
        let mut stop = &self.stops[stop_idx];

        let mut last_color = alpha_mul(stop.color.val, alpha);
        let mut last_pos = 0;

        let mut next_color = last_color;
        let mut next_pos = (255. * stop.position) as u32;

        let mut i = 0;

        const FIXED_SHIFT: u32 = 8;
        const FIXED_ONE: u32 = 1 << FIXED_SHIFT;
        const FIXED_HALF: u32 = FIXED_ONE >> 1;

        while i <= 255 {
            while next_pos <= i {
                stop_idx += 1;
                last_color = next_color;
                if stop_idx >= self.stops.len() {
                    stop = &self.stops[self.stops.len() - 1];
                    next_pos = 255;
                    next_color = alpha_mul(stop.color.val, alpha);
                    break;
                } else {
                    stop = &self.stops[stop_idx];
                }
                next_pos = (255. * stop.position) as u32;
                next_color = alpha_mul(stop.color.val, alpha);
            }
            let inverse = (FIXED_ONE * 256) / (next_pos - last_pos);
            let mut t = 0;
            // XXX we could actually avoid doing any multiplications inside
            // this loop by accumulating (next_color - last_color)*inverse
            while i <= next_pos {
                // stops need to be represented in unpremultipled form otherwise we lose information
                // that we need when lerping between colors
                lut[i as usize] = premultiply(lerp(last_color, next_color, (t + FIXED_HALF) >> FIXED_SHIFT));
                t += inverse;
                i += 1;
            }
            last_pos = next_pos;
        }
    }
}

pub trait PixelFetch {
    fn get_pixel(bitmap: &Image,  x: i32,  y: i32) -> u32;
}


pub struct PadFetch;
impl PixelFetch for PadFetch {
    fn get_pixel(bitmap: &Image, mut x: i32, mut y: i32) -> u32 {
        if x < 0 {
            x = 0;
        }
        if x >= bitmap.width {
            x = bitmap.width - 1;
        }

        if y < 0 {
            y = 0;
        }
        if y >= bitmap.height {
            y = bitmap.height - 1;
        }

        return bitmap.data[(y * bitmap.width + x) as usize];
    }
}

pub struct RepeatFetch;
impl PixelFetch for RepeatFetch {
    fn get_pixel(bitmap: &Image, mut x: i32, mut y: i32) -> u32 {

        // XXX: This is a very slow approach to repeating.
        // We should instead do the wrapping in the iterator
        x = x % bitmap.width;
        if x < 0 {
            x = x + bitmap.width;
        }

        y = y % bitmap.height;
        if y < 0 {
            y = y + bitmap.height;
        }

        return bitmap.data[(y * bitmap.width + x) as usize];
    }
}


/* Inspired by Filter_32_opaque from Skia */
fn bilinear_interpolation(
    tl: u32,
    tr: u32,
    bl: u32,
    br: u32,
    mut distx: u32,
    mut disty: u32,
) -> u32 {
    let distxy;
    let distxiy;
    let distixy;
    let distixiy;
    let mut lo;
    let mut hi;

    distx <<= 4 - BILINEAR_INTERPOLATION_BITS;
    disty <<= 4 - BILINEAR_INTERPOLATION_BITS;

    distxy = distx * disty;
    distxiy = (distx << 4) - distxy; /* distx * (16 - disty) */
    distixy = (disty << 4) - distxy; /* disty * (16 - distx) */

    /* (16 - distx) * (16 - disty) */
    // The intermediate calculation can underflow so we use
    // wrapping arithmetic to let the compiler know that it's ok
    distixiy = (16u32 * 16)
        .wrapping_sub(disty << 4)
        .wrapping_sub(distx << 4)
        .wrapping_add(distxy);

    lo = (tl & 0xff00ff) * distixiy;
    hi = ((tl >> 8) & 0xff00ff) * distixiy;

    lo += (tr & 0xff00ff) * distxiy;
    hi += ((tr >> 8) & 0xff00ff) * distxiy;

    lo += (bl & 0xff00ff) * distixy;
    hi += ((bl >> 8) & 0xff00ff) * distixy;

    lo += (br & 0xff00ff) * distxy;
    hi += ((br >> 8) & 0xff00ff) * distxy;

    ((lo >> 8) & 0xff00ff) | (hi & !0xff00ff)
}

/* Inspired by Filter_32_alpha from Skia */
fn bilinear_interpolation_alpha(
    tl: u32,
    tr: u32,
    bl: u32,
    br: u32,
    mut distx: u32,
    mut disty: u32,
    alpha: Alpha256
) -> u32 {
    let distxy;
    let distxiy;
    let distixy;
    let distixiy;
    let mut lo;
    let mut hi;

    distx <<= 4 - BILINEAR_INTERPOLATION_BITS;
    disty <<= 4 - BILINEAR_INTERPOLATION_BITS;

    distxy = distx * disty;
    distxiy = (distx << 4) - distxy; /* distx * (16 - disty) */
    distixy = (disty << 4) - distxy; /* disty * (16 - distx) */
     /* (16 - distx) * (16 - disty) */
    // The intermediate calculation can underflow so we use
    // wrapping arithmetic to let the compiler know that it's ok
    distixiy = (16u32 * 16)
        .wrapping_sub(disty << 4)
        .wrapping_sub(distx << 4)
        .wrapping_add(distxy);

    lo = (tl & 0xff00ff) * distixiy;
    hi = ((tl >> 8) & 0xff00ff) * distixiy;

    lo += (tr & 0xff00ff) * distxiy;
    hi += ((tr >> 8) & 0xff00ff) * distxiy;

    lo += (bl & 0xff00ff) * distixy;
    hi += ((bl >> 8) & 0xff00ff) * distixy;

    lo += (br & 0xff00ff) * distxy;
    hi += ((br >> 8) & 0xff00ff) * distxy;

    lo = ((lo >> 8) & 0xff00ff) * alpha;
    hi = ((hi >> 8) & 0xff00ff) * alpha;

    ((lo >> 8) & 0xff00ff) | (hi & !0xff00ff)
}

const FIXED_FRACTION_BITS: u32 = 16;
pub const FIXED_ONE: i32 = 1 << FIXED_FRACTION_BITS;

fn bilinear_weight(x: Fixed) -> u32 {
    // discard the unneeded bits of precision
    let reduced = x >> (FIXED_FRACTION_BITS - BILINEAR_INTERPOLATION_BITS);
    // extract the remaining fraction
    let fraction = reduced & ((1 << BILINEAR_INTERPOLATION_BITS) - 1);
    fraction as u32
}

type Fixed = i32;

fn fixed_to_int(x: Fixed) -> i32 {
    x >> FIXED_FRACTION_BITS
}

// there are various tricks the can be used
// to make this faster. Let's just do simplest
// thing for now
pub fn float_to_fixed(x: f32) -> Fixed {
    ((x * (1 << FIXED_FRACTION_BITS) as f32) + 0.5) as i32
}

pub fn fetch_bilinear<Fetch: PixelFetch>(image: &Image, x: Fixed, y: Fixed) -> u32 {
    let dist_x = bilinear_weight(x);
    let dist_y = bilinear_weight(y);

    let x1 = fixed_to_int(x);
    let y1 = fixed_to_int(y);
    let x2 = x1 + 1;
    let y2 = y1 + 1;

    let tl = Fetch::get_pixel(image, x1, y1);
    let tr = Fetch::get_pixel(image, x2, y1);
    let bl = Fetch::get_pixel(image, x1, y2);
    let br = Fetch::get_pixel(image, x2, y2);

    bilinear_interpolation(tl, tr, bl, br, dist_x, dist_y)
}

pub fn fetch_bilinear_alpha<Fetch: PixelFetch>(image: &Image, x: Fixed, y: Fixed, alpha: Alpha256) -> u32 {
    let dist_x = bilinear_weight(x);
    let dist_y = bilinear_weight(y);

    let x1 = fixed_to_int(x);
    let y1 = fixed_to_int(y);
    let x2 = x1 + 1;
    let y2 = y1 + 1;

    let tl = Fetch::get_pixel(image, x1, y1);
    let tr = Fetch::get_pixel(image, x2, y1);
    let bl = Fetch::get_pixel(image, x1, y2);
    let br = Fetch::get_pixel(image, x2, y2);

    bilinear_interpolation_alpha(tl, tr, bl, br, dist_x, dist_y, alpha)
}

pub fn fetch_nearest<Fetch: PixelFetch>(image: &Image, x: Fixed, y: Fixed) -> u32 {
    Fetch::get_pixel(image, fixed_to_int(x), fixed_to_int(y))
}

pub fn fetch_nearest_alpha<Fetch: PixelFetch>(image: &Image, x: Fixed, y: Fixed, alpha: Alpha256) -> u32 {
    alpha_mul(Fetch::get_pixel(image, fixed_to_int(x), fixed_to_int(y)), alpha)
}

pub struct PointFixedPoint {
    pub x: Fixed,
    pub y: Fixed,
}

#[derive(Clone)]
pub struct MatrixFixedPoint {
    pub xx: Fixed,
    pub xy: Fixed,
    pub yx: Fixed,
    pub yy: Fixed,
    pub x0: Fixed,
    pub y0: Fixed,
}

impl MatrixFixedPoint {
    pub fn transform(&self, x: u16, y: u16) -> PointFixedPoint {
        let x = x as i32;
        let y = y as i32;
        // when taking integer parameters we can use a regular mulitply instead of a fixed one
        PointFixedPoint {
            x: x * self.xx + self.xy * y + self.x0,
            y: y * self.yy + self.yx * x + self.y0,
        }
    }
}

fn premultiply(c: u32) -> u32 {
    // This could be optimized by using SWAR
    let a = get_packed_a32(c);
    let mut r = get_packed_r32(c);
    let mut g = get_packed_g32(c);
    let mut b = get_packed_b32(c);

    if a < 255 {
        r = muldiv255(r, a);
        g = muldiv255(g, a);
        b = muldiv255(b, a);
    }

    pack_argb32(a, r, g, b)
}

fn pack_argb32(a: u32, r: u32, g: u32, b: u32) -> u32 {
    debug_assert!(r <= a);
    debug_assert!(g <= a);
    debug_assert!(b <= a);

    return (a << A32_SHIFT) | (r << R32_SHIFT) |
        (g << G32_SHIFT) | (b << B32_SHIFT);
}

fn get_packed_a32(packed: u32) -> u32 { ((packed) << (24 - A32_SHIFT)) >> 24 }
fn get_packed_r32(packed: u32) -> u32 { ((packed) << (24 - R32_SHIFT)) >> 24 }
fn get_packed_g32(packed: u32) -> u32 { ((packed) << (24 - G32_SHIFT)) >> 24 }
fn get_packed_b32(packed: u32) -> u32 { ((packed) << (24 - B32_SHIFT)) >> 24 }

#[inline]
fn packed_alpha(x: u32) -> u32 {
    x >> A32_SHIFT
}

// this is an approximation of true 'over' that does a division by 256 instead
// of 255. It is the same style of blending that Skia does.
pub fn over(src: u32, dst: u32) -> u32 {
    let a = packed_alpha(src);
    let a = 256 - a;
    let mask = 0xff00ff;
    let rb = ((dst & 0xff00ff) * a) >> 8;
    let ag = ((dst >> 8) & 0xff00ff) * a;
    src + (rb & mask) | (ag & !mask)
}

#[inline]
pub fn alpha_to_alpha256(alpha: u32) -> u32 {
    alpha + 1
}

/** Calculates 256 - (value * alpha256) / 255 in range [0,256],
 *  for [0,255] value and [0,256] alpha256. */
#[inline]
fn alpha_mul_inv256(value: u32, alpha256: u32) -> u32 {
    let prod = 0xFFFF - value * alpha256;
    return (prod + (prod >> 8)) >> 8;
}

/** Calculates (value * alpha256) / 255 in range [0,256],
 *  for [0,255] value and [0,256] alpha256. */
fn alpha_mul_256(value: u32, alpha256: u32) -> u32 {
    let prod = value * alpha256;
    return (prod + (prod >> 8)) >> 8;
}

pub fn muldiv255(a: u32, b: u32) -> u32 {
    let tmp = a * b + 128;
    ((tmp + (tmp >> 8)) >> 8)
}

pub fn div255(a: u32) -> u32 {
    let tmp = a + 128;
    ((tmp + (tmp >> 8)) >> 8)
}

#[inline]
pub fn alpha_mul(x: u32, a: Alpha256) -> u32 {
    let mask = 0xFF00FF;

    let src_rb = ((x & mask) * a) >> 8;
    let src_ag = ((x >> 8) & mask) * a;

    return (src_rb & mask) | (src_ag & !mask)
}

// This approximates the division by 255 using a division by 256.
// It matches the behaviour of SkBlendARGB32 from Skia in 2017.
// The behaviour of this function was changed in 2016 by Lee Salzman
// in Skia:40254c2c2dc28a34f96294d5a1ad94a99b0be8a6 to keep more of the
// intermediate precision
#[inline]
pub fn over_in(src: u32, dst: u32, alpha: u32) -> u32 {
    let src_alpha = alpha_to_alpha256(alpha);
    let dst_alpha = alpha_mul_inv256(packed_alpha(src), src_alpha);

    let mask = 0xFF00FF;

    let src_rb = (src & mask) * src_alpha;
    let src_ag = ((src >> 8) & mask) * src_alpha;

    let dst_rb = (dst & mask) * dst_alpha;
    let dst_ag = ((dst >> 8) & mask) * dst_alpha;

    // we sum src and dst before reducing to 8 bit to avoid accumulating rounding errors
    return (((src_rb + dst_rb) >> 8) & mask) | ((src_ag + dst_ag) & !mask);
}

// Similar to over_in but includes an additional clip alpha value
#[inline]
pub fn over_in_in(src: u32, dst: u32, mask: u32, clip: u32) -> u32 {
    let src_alpha = alpha_to_alpha256(mask);
    let src_alpha = alpha_mul_256(src_alpha, clip);
    let dst_alpha = alpha_mul_inv256(packed_alpha(src), src_alpha);

    let mask = 0xFF00FF;

    let src_rb = (src & mask) * src_alpha;
    let src_ag = ((src >> 8) & mask) * src_alpha;

    let dst_rb = (dst & mask) * dst_alpha;
    let dst_ag = ((dst >> 8) & mask) * dst_alpha;

    // we sum src and dst before reducing to 8 bit to avoid accumulating rounding errors
    return (((src_rb + dst_rb) >> 8) & mask) | ((src_ag + dst_ag) & !mask);
}

pub fn alpha_lerp(src: u32, dst: u32, mask: u32, clip: u32) -> u32 {
    let alpha = alpha_mul_256(alpha_to_alpha256(mask), clip);
    return lerp(src, dst, alpha);
}
