use std::f32::consts::TAU;

use windows_sys::Win32::Graphics::OpenGL::*;

use crate::scene::{Frame, Scene, Viewport};

const GROWTH_SECONDS: f32 = 240.0;
const HOLD_SECONDS: f32 = 7.0;
const NEW_POINT_GLOW_SECONDS: f32 = 0.85;
const MIN_ZOOM_RADIUS: f32 = 9.0;
const CANVAS_HALF_EXTENT: f32 = 0.42;
const CURVE_SAMPLE_COUNT: usize = 256;
const CURVE_PANEL_MARGIN: f32 = 24.0;

#[derive(Clone, Copy)]
struct Color {
    red: f32,
    green: f32,
    blue: f32,
}

impl Color {
    const fn new(red: f32, green: f32, blue: f32) -> Self {
        Self { red, green, blue }
    }

    fn scaled(self, amount: f32) -> Self {
        Self::new(self.red * amount, self.green * amount, self.blue * amount)
    }

    fn mix(self, other: Self, amount: f32) -> Self {
        Self::new(
            self.red + (other.red - self.red) * amount,
            self.green + (other.green - self.green) * amount,
            self.blue + (other.blue - self.blue) * amount,
        )
    }
}

#[derive(Clone, Copy)]
struct Palette {
    shadow: Color,
    deep: Color,
    body: Color,
    accent: Color,
}

const PALETTES: [Palette; 4] = [
    Palette {
        shadow: Color::new(0.003, 0.006, 0.018),
        deep: Color::new(0.012, 0.036, 0.086),
        body: Color::new(0.08, 0.66, 0.94),
        accent: Color::new(0.96, 0.34, 0.68),
    },
    Palette {
        shadow: Color::new(0.012, 0.004, 0.020),
        deep: Color::new(0.074, 0.016, 0.106),
        body: Color::new(0.68, 0.24, 0.96),
        accent: Color::new(1.00, 0.68, 0.22),
    },
    Palette {
        shadow: Color::new(0.002, 0.014, 0.016),
        deep: Color::new(0.008, 0.080, 0.074),
        body: Color::new(0.12, 0.91, 0.66),
        accent: Color::new(0.84, 0.98, 0.49),
    },
    Palette {
        shadow: Color::new(0.018, 0.007, 0.004),
        deep: Color::new(0.100, 0.026, 0.010),
        body: Color::new(1.00, 0.37, 0.13),
        accent: Color::new(1.00, 0.88, 0.58),
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldShape {
    Square,
    Circle,
}

impl FieldShape {
    const fn from_seed(seed: u32) -> Self {
        if seed & 1 == 0 {
            Self::Square
        } else {
            Self::Circle
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct UlamPoint {
    number: u32,
    x: f32,
    y: f32,
    phase: f32,
    energy: f32,
}

/// A prime-count sample is kept separate from the Ulam coordinates so the
/// graph can show the exact step function π(n), not a visual fit to it.
#[derive(Clone, Copy, Debug)]
struct PrimeCountPoint {
    number: u32,
    count: u32,
}

#[derive(Clone, Copy, Debug)]
struct SmoothCountPoint {
    number: u32,
    estimate: f32,
}

struct PrimeCountCurve {
    maximum_number: u32,
    exact: Vec<PrimeCountPoint>,
    logarithmic_integral: Vec<SmoothCountPoint>,
}

struct PrimeFrame {
    palette: Palette,
    center: (f32, f32),
    scale: f32,
    time: f32,
    lit_number: u32,
    field_radius: f32,
}

/// Prime Field keeps its natural-number reveal rule inside the field. The
/// shared host only supplies monotonic elapsed time and a GL context.
pub struct PrimeScene {
    viewport: Viewport,
    preview: bool,
    scene_started_at: f32,
    scene_index: usize,
    scene_seed: u32,
    shape: FieldShape,
    maximum_side: u32,
    points: Vec<UlamPoint>,
    count_curve: PrimeCountCurve,
}

impl PrimeScene {
    pub fn new(preview: bool) -> Self {
        let mut scene = Self {
            viewport: Viewport {
                width: 1,
                height: 1,
            },
            preview,
            scene_started_at: 0.0,
            scene_index: 0,
            scene_seed: 0x8b34_75c1,
            shape: FieldShape::Square,
            maximum_side: 0,
            points: Vec::new(),
            count_curve: PrimeCountCurve::default(),
        };
        scene.rebuild();
        scene
    }

    fn rebuild(&mut self) {
        const FULL_FIELD_SIDES: [u32; 4] = [251, 301, 351, 401];
        self.maximum_side = if self.preview {
            151
        } else {
            FULL_FIELD_SIDES[self.scene_index % FULL_FIELD_SIDES.len()]
        };
        self.shape =
            FieldShape::from_seed(self.scene_seed ^ (self.scene_index as u32).rotate_left(7));
        self.points = generate_points(self.maximum_side, self.scene_seed);
        self.count_curve =
            generate_prime_count_curve(&self.points, self.maximum_side * self.maximum_side);
    }

    fn advance_scene_if_needed(&mut self, elapsed_seconds: f32) {
        if self.preview || elapsed_seconds - self.scene_started_at < GROWTH_SECONDS + HOLD_SECONDS {
            return;
        }
        self.scene_started_at = elapsed_seconds;
        self.scene_index = (self.scene_index + 1) % PALETTES.len();
        self.scene_seed = self
            .scene_seed
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.rebuild();
    }

    fn revealed_number(&self, scene_seconds: f32) -> f32 {
        let maximum = (self.maximum_side * self.maximum_side) as f32;
        (scene_seconds / GROWTH_SECONDS).clamp(0.0, 1.0) * maximum
    }

    unsafe fn draw_background(&self, palette: Palette, width: f32, height: f32, time: f32) {
        let drift = 0.11 + 0.06 * (time * 0.043).sin();
        unsafe {
            glClearColor(
                palette.shadow.red,
                palette.shadow.green,
                palette.shadow.blue,
                1.0,
            );
            glClear(GL_COLOR_BUFFER_BIT);
            glBegin(GL_QUADS);
            glColor3f(
                palette.shadow.red,
                palette.shadow.green,
                palette.shadow.blue,
            );
            glVertex2f(0.0, 0.0);
            let upper = palette.deep.scaled(0.48 + drift);
            glColor3f(upper.red, upper.green, upper.blue);
            glVertex2f(width, 0.0);
            let lower = palette.deep.scaled(0.23 + drift * 0.45);
            glColor3f(lower.red, lower.green, lower.blue);
            glVertex2f(width, height);
            glColor3f(
                palette.shadow.red,
                palette.shadow.green,
                palette.shadow.blue,
            );
            glVertex2f(0.0, height);
            glEnd();
        }
    }

    unsafe fn draw_bloom(
        &self,
        palette: Palette,
        center: (f32, f32),
        width: f32,
        height: f32,
        time: f32,
    ) {
        let radius = width.max(height) * (0.55 + 0.05 * (time * 0.071).sin());
        let offset = (time * 0.11).sin() * width * 0.06;
        unsafe {
            glBlendFunc(GL_SRC_ALPHA, GL_ONE);
            glBegin(GL_TRIANGLE_FAN);
            let core = palette.body.scaled(0.045);
            glColor4f(core.red, core.green, core.blue, 0.38);
            glVertex2f(center.0 + offset, center.1 - offset * 0.25);
            for index in 0..=48 {
                let angle = TAU * index as f32 / 48.0;
                let edge = palette.deep.scaled(0.12);
                glColor4f(edge.red, edge.green, edge.blue, 0.0);
                glVertex2f(
                    center.0 + offset + angle.cos() * radius,
                    center.1 - offset * 0.25 + angle.sin() * radius,
                );
            }
            glEnd();
        }
    }

    unsafe fn draw_grid(&self, palette: Palette, center: (f32, f32), scale: f32, scene_time: f32) {
        let shade = palette.body.scaled(0.17 + 0.04 * (scene_time * 0.16).sin());
        unsafe {
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            glLineWidth(1.0);
            glColor4f(shade.red, shade.green, shade.blue, 0.17);
            glBegin(GL_LINES);
            for index in -8..=8 {
                let position = index as f32 / 8.0;
                glVertex2f(center.0 + position * scale, center.1 - scale);
                glVertex2f(center.0 + position * scale, center.1 + scale);
                glVertex2f(center.0 - scale, center.1 + position * scale);
                glVertex2f(center.0 + scale, center.1 + position * scale);
            }
            glEnd();
        }
    }

    unsafe fn draw_orbits(&self, palette: Palette, center: (f32, f32), scale: f32, time: f32) {
        let color = palette.accent.scaled(0.22);
        unsafe {
            glBlendFunc(GL_SRC_ALPHA, GL_ONE);
            glLineWidth(1.0);
            for ring in 1..=3 {
                let radius =
                    scale * (0.17 + ring as f32 * 0.18 + 0.012 * (time * 0.4 + ring as f32).sin());
                glColor4f(color.red, color.green, color.blue, 0.095 / ring as f32);
                glBegin(GL_LINE_STRIP);
                for index in 0..=96 {
                    let theta = TAU * index as f32 / 96.0 - time * 0.045 * ring as f32;
                    glVertex2f(
                        center.0 + theta.cos() * radius,
                        center.1 + theta.sin() * radius,
                    );
                }
                glEnd();
            }
        }
    }

    unsafe fn draw_origin(&self, palette: Palette, center: (f32, f32), scale: f32, time: f32) {
        let glow = palette.accent.scaled(0.55 + 0.25 * (time * 1.7).sin());
        unsafe {
            glBlendFunc(GL_SRC_ALPHA, GL_ONE);
            glPointSize((scale * 2.8).clamp(4.0, 15.0));
            glBegin(GL_POINTS);
            glColor4f(glow.red, glow.green, glow.blue, 0.11);
            glVertex2f(center.0, center.1);
            glEnd();
            glPointSize((scale * 0.75).clamp(1.5, 5.0));
            glBegin(GL_POINTS);
            glColor4f(glow.red, glow.green, glow.blue, 0.85);
            glVertex2f(center.0, center.1);
            glEnd();
        }
    }

    unsafe fn draw_primes(&self, frame: PrimeFrame) {
        let visible_count = self
            .points
            .partition_point(|point| point.number <= frame.lit_number);
        let core_size = (frame.scale * 0.55).clamp(1.0, 5.6);
        let bloom_size = (core_size * 3.3).clamp(3.3, 11.0);
        let numbers_per_second = self.maximum_side * self.maximum_side;
        let numbers_per_second = numbers_per_second as f32 / GROWTH_SECONDS;
        unsafe {
            if bloom_size - core_size > 0.35 {
                glBlendFunc(GL_SRC_ALPHA, GL_ONE);
                glPointSize(bloom_size);
                glBegin(GL_POINTS);
                for point in &self.points[..visible_count] {
                    if self.shape == FieldShape::Circle
                        && point.x * point.x + point.y * point.y
                            > frame.field_radius * frame.field_radius
                    {
                        continue;
                    }
                    let freshness = point_freshness(*point, frame.lit_number, numbers_per_second);
                    let (color, pulse) = point_color(*point, frame.palette, frame.time, freshness);
                    let (x, y) = project((point.x, point.y), frame.center, frame.scale);
                    glColor4f(
                        color.red,
                        color.green,
                        color.blue,
                        0.035 + pulse * 0.075 + freshness * 0.30,
                    );
                    glVertex2f(x, y);
                }
                glEnd();
            }

            glPointSize(core_size);
            glBegin(GL_POINTS);
            for point in &self.points[..visible_count] {
                if self.shape == FieldShape::Circle
                    && point.x * point.x + point.y * point.y
                        > frame.field_radius * frame.field_radius
                {
                    continue;
                }
                let freshness = point_freshness(*point, frame.lit_number, numbers_per_second);
                let (color, pulse) = point_color(*point, frame.palette, frame.time, freshness);
                let (x, y) = project((point.x, point.y), frame.center, frame.scale);
                glColor4f(
                    color.red,
                    color.green,
                    color.blue,
                    0.30 + pulse * 0.55 + freshness * 0.15,
                );
                glVertex2f(x, y);
            }
            glEnd();
        }
    }

    unsafe fn draw_prime_count_curve(&self, width: f32, height: f32, lit_number: u32) {
        if width < 640.0 || height < 400.0 {
            return;
        }
        let panel_width = (width * 0.285).clamp(250.0, 380.0);
        let panel_height = (height * 0.205).clamp(116.0, 172.0);
        let panel_left = CURVE_PANEL_MARGIN;
        let panel_top = height - panel_height - CURVE_PANEL_MARGIN;
        let inset = 11.0;
        let graph_left = panel_left + inset;
        let graph_top = panel_top + inset;
        let graph_width = panel_width - inset * 2.0;
        let graph_height = panel_height - inset * 2.0;
        let curve = &self.count_curve;
        let cursor = lit_number.min(curve.maximum_number);
        let current_count = curve.exact.partition_point(|point| point.number <= cursor) as f32;
        // Scale to the revealed range. A final-range scale makes the genuine
        // π(n)-Li(n) separation sub-pixel through most of the animation.
        let visible_scale = current_count
            .max(curve.logarithmic_integral_at(cursor))
            .max(2.0)
            * 1.08;
        let project = |number: u32, count: f32| {
            (
                graph_left
                    + (number.saturating_sub(2) as f32 / cursor.saturating_sub(2).max(1) as f32)
                        * graph_width,
                graph_top + graph_height - count / visible_scale * graph_height,
            )
        };
        unsafe {
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            glColor4f(0.004, 0.016, 0.038, 0.86);
            glBegin(GL_QUADS);
            glVertex2f(panel_left, panel_top);
            glVertex2f(panel_left + panel_width, panel_top);
            glVertex2f(panel_left + panel_width, panel_top + panel_height);
            glVertex2f(panel_left, panel_top + panel_height);
            glEnd();

            glLineWidth(1.0);
            glColor4f(0.30, 0.70, 0.92, 0.28);
            glBegin(GL_LINE_LOOP);
            glVertex2f(panel_left, panel_top);
            glVertex2f(panel_left + panel_width, panel_top);
            glVertex2f(panel_left + panel_width, panel_top + panel_height);
            glVertex2f(panel_left, panel_top + panel_height);
            glEnd();
            glBegin(GL_LINES);
            for division in 1..4 {
                let y = graph_top + graph_height * division as f32 / 4.0;
                glVertex2f(graph_left, y);
                glVertex2f(graph_left + graph_width, y);
            }
            glEnd();

            // The amber line is the classical logarithmic-integral estimate
            // Li(n), not an adjustable numerical fit. Both curves are
            // evaluated only through the current natural-number frontier. The
            // horizontal axis expands with that frontier, keeping the visible
            // π(n)-Li(n) separation readable during the reveal.
            glBlendFunc(GL_SRC_ALPHA, GL_ONE);
            glLineWidth(2.1);
            glColor4f(1.0, 0.66, 0.28, 0.92);
            glBegin(GL_LINE_STRIP);
            let visible_estimates = curve
                .logarithmic_integral
                .iter()
                .copied()
                .take_while(|point| point.number <= cursor)
                .collect::<Vec<_>>();
            for point in &visible_estimates {
                let (x, y) = project(point.number, point.estimate);
                glVertex2f(x, y);
            }
            if let Some(last) = visible_estimates.last().copied()
                && last.number < cursor
            {
                let (start_x, start_y) = project(last.number, last.estimate);
                let (end_x, end_y) = project(cursor, curve.logarithmic_integral_at(cursor));
                glVertex2f(start_x, start_y);
                glVertex2f(end_x, end_y);
            }
            glEnd();

            // π(n) is exactly constant between primes and increases by one at
            // every prime, so it is deliberately drawn as a staircase.
            glLineWidth(1.35);
            glColor4f(0.28, 0.84, 0.98, 0.95);
            glBegin(GL_LINE_STRIP);
            let mut previous_count = 0.0;
            let (x, y) = project(2, previous_count);
            glVertex2f(x, y);
            for point in &curve.exact {
                if point.number > cursor {
                    break;
                }
                let (before_x, before_y) = project(point.number, previous_count);
                glVertex2f(before_x, before_y);
                let (_, after_y) = project(point.number, point.count as f32);
                glVertex2f(before_x, after_y);
                previous_count = point.count as f32;
            }
            let (end_x, end_y) = project(cursor, previous_count);
            glVertex2f(end_x, end_y);
            glEnd();
        }
    }
}

impl Default for PrimeCountCurve {
    fn default() -> Self {
        Self {
            maximum_number: 2,
            exact: Vec::new(),
            logarithmic_integral: Vec::new(),
        }
    }
}

impl PrimeCountCurve {
    fn logarithmic_integral_at(&self, number: u32) -> f32 {
        let index = self
            .logarithmic_integral
            .partition_point(|point| point.number <= number);
        match (
            self.logarithmic_integral.get(index.saturating_sub(1)),
            self.logarithmic_integral.get(index),
        ) {
            (Some(lower), Some(upper)) if upper.number > lower.number => {
                let fraction =
                    (number - lower.number) as f32 / (upper.number - lower.number) as f32;
                lower.estimate + (upper.estimate - lower.estimate) * fraction
            }
            (Some(lower), _) => lower.estimate,
            _ => 0.0,
        }
    }
}

impl Scene for PrimeScene {
    unsafe fn activate(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn resize(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }

    unsafe fn render(&mut self, frame: Frame) -> bool {
        self.advance_scene_if_needed(frame.elapsed_seconds);
        let scene_time = frame.elapsed_seconds - self.scene_started_at;
        let palette = PALETTES[self.scene_index % PALETTES.len()];
        let width = self.viewport.width as f32;
        let height = self.viewport.height as f32;
        let center = (width * 0.5, height * 0.5);
        let revealed_number = self.revealed_number(scene_time);
        let lit_number = (revealed_number.floor() as u32).max(2);
        // A side of sqrt(n) contains n natural numbers. The half-extent is
        // sqrt(n)/2, so scale is exactly proportional to 1/sqrt(n). The
        // rendering does not depend on the count of primes and never clamps to
        // an integer ring, avoiding visible per-ring zoom steps.
        let canvas_radius = width.min(height) * CANVAS_HALF_EXTENT;
        let field_radius = (revealed_number.sqrt() * 0.5).max(1.0);
        let scale = canvas_radius / field_radius.max(MIN_ZOOM_RADIUS);
        unsafe {
            glViewport(0, 0, self.viewport.width, self.viewport.height);
            glMatrixMode(GL_PROJECTION);
            glLoadIdentity();
            glOrtho(0.0, width as f64, height as f64, 0.0, -1.0, 1.0);
            glMatrixMode(GL_MODELVIEW);
            glLoadIdentity();
            glDisable(GL_BLEND);
            self.draw_background(palette, width, height, frame.elapsed_seconds);
            glEnable(GL_BLEND);
            self.draw_bloom(palette, center, width, height, frame.elapsed_seconds);
            self.draw_grid(palette, center, canvas_radius, scene_time);
            self.draw_orbits(palette, center, canvas_radius, frame.elapsed_seconds);
            self.draw_origin(palette, center, scale, frame.elapsed_seconds);
            self.draw_primes(PrimeFrame {
                palette,
                center,
                scale,
                time: frame.elapsed_seconds,
                lit_number,
                field_radius,
            });
            self.draw_prime_count_curve(width, height, lit_number);
            glDisable(GL_BLEND);
        }
        true
    }

    unsafe fn prepare_present(&mut self, _frame: Frame) {}

    unsafe fn presented(&mut self, _frame: Frame, _succeeded: bool) {}

    unsafe fn deactivate(&mut self) {}
}

fn generate_prime_count_curve(points: &[UlamPoint], maximum_number: u32) -> PrimeCountCurve {
    const LI_AT_TWO: f32 = 1.045_163_8;
    let mut exact = Vec::with_capacity(points.len());
    for (index, point) in points.iter().enumerate() {
        exact.push(PrimeCountPoint {
            number: point.number,
            count: (index + 1) as u32,
        });
    }

    let sample_count = CURVE_SAMPLE_COUNT
        .min(maximum_number.saturating_sub(1) as usize)
        .max(2);
    let mut logarithmic_integral = Vec::with_capacity(sample_count);
    let mut sample_index = 0;
    let mut next_sample = 2_u32;
    let mut integral = LI_AT_TWO;
    for number in 2..=maximum_number {
        while sample_index < sample_count && number >= next_sample {
            logarithmic_integral.push(SmoothCountPoint {
                number,
                estimate: integral,
            });
            sample_index += 1;
            next_sample = 2
                + ((maximum_number.saturating_sub(2) as u64 * sample_index as u64)
                    / (sample_count.saturating_sub(1) as u64)) as u32;
        }
        if number < maximum_number {
            let left = number as f32;
            let right = (number + 1) as f32;
            // Trapezoidal integration of 1/log x from 2 to n approximates the
            // offset logarithmic integral Li(n) without its singularity at 1.
            integral += 0.5 * (1.0 / left.ln() + 1.0 / right.ln());
        }
    }
    if logarithmic_integral
        .last()
        .is_none_or(|point| point.number != maximum_number)
    {
        logarithmic_integral.push(SmoothCountPoint {
            number: maximum_number,
            estimate: integral,
        });
    }
    PrimeCountCurve {
        maximum_number,
        exact,
        logarithmic_integral,
    }
}

fn project(point: (f32, f32), center: (f32, f32), scale: f32) -> (f32, f32) {
    (center.0 + point.0 * scale, center.1 + point.1 * scale)
}

fn point_freshness(point: UlamPoint, lit_number: u32, numbers_per_second: f32) -> f32 {
    let age_in_numbers = lit_number.saturating_sub(point.number) as f32;
    (1.0 - age_in_numbers / (numbers_per_second * NEW_POINT_GLOW_SECONDS)).clamp(0.0, 1.0)
}

fn point_color(point: UlamPoint, palette: Palette, time: f32, freshness: f32) -> (Color, f32) {
    let wave = 0.5 + 0.5 * (point.phase * TAU * 2.0 - time * 0.72).sin();
    let ripple = 0.5 + 0.5 * (point.phase * TAU * 9.0 + time * 1.17).sin();
    let color = palette
        .body
        .mix(palette.accent, wave * 0.76)
        .scaled(point.energy * (0.62 + ripple * 0.48) * (1.0 + freshness * 1.15));
    (color, ripple)
}

fn generate_points(side: u32, seed: u32) -> Vec<UlamPoint> {
    let side = side.clamp(3, 801) | 1;
    let largest = side * side;
    let mut prime = vec![true; (largest + 1) as usize];
    prime[0] = false;
    prime[1] = false;
    let mut divisor = 2_u32;
    while divisor * divisor <= largest {
        if prime[divisor as usize] {
            let mut multiple = divisor * divisor;
            while multiple <= largest {
                prime[multiple as usize] = false;
                multiple += divisor;
            }
        }
        divisor += 1;
    }
    let mut points = Vec::with_capacity((largest as f32 / (largest as f32).ln()) as usize);
    for number in 2..=largest {
        if !prime[number as usize] {
            continue;
        }
        let (x, y) = spiral_coordinate(number);
        let hash = mix_bits(number ^ seed.rotate_left(11));
        let phase = hash as f32 * (1.0 / u32::MAX as f32);
        let normalized_age = number as f32 / largest as f32;
        points.push(UlamPoint {
            number,
            x: x as f32,
            y: y as f32,
            phase,
            energy: 0.42 + 0.58 * (1.0 - normalized_age).powf(0.22),
        });
    }
    points
}

fn spiral_coordinate(number: u32) -> (i32, i32) {
    if number <= 1 {
        return (0, 0);
    }
    let layer = (((number as f64).sqrt() - 1.0) * 0.5).ceil() as i32;
    let odd_side = layer * 2 + 1;
    let corner = odd_side * odd_side;
    let distance = corner - number as i32;
    let side = layer * 2;
    if distance < side {
        (layer - distance, -layer)
    } else if distance < side * 2 {
        (-layer, -layer + (distance - side))
    } else if distance < side * 3 {
        (-layer + (distance - side * 2), layer)
    } else {
        (layer, layer - (distance - side * 3))
    }
}

fn mix_bits(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

#[cfg(test)]
mod tests {
    use super::{
        FieldShape, GROWTH_SECONDS, PrimeScene, generate_points, generate_prime_count_curve,
        spiral_coordinate,
    };

    #[test]
    fn first_spiral_turn_has_the_expected_coordinates() {
        let expected = [
            (0, 0),
            (1, 0),
            (1, 1),
            (0, 1),
            (-1, 1),
            (-1, 0),
            (-1, -1),
            (0, -1),
            (1, -1),
        ];
        for (number, coordinate) in (1..=9).zip(expected) {
            assert_eq!(spiral_coordinate(number), coordinate);
        }
    }

    #[test]
    fn sieve_keeps_the_nine_primes_through_twenty_five() {
        assert_eq!(generate_points(5, 7).len(), 9);
    }

    #[test]
    fn even_requested_side_becomes_an_odd_square() {
        assert_eq!(generate_points(4, 7).len(), 9);
    }

    #[test]
    fn reveal_cursor_advances_with_natural_numbers_not_prime_count() {
        let scene = PrimeScene::new(false);
        assert_eq!(
            scene.revealed_number(GROWTH_SECONDS * 0.25),
            251.0_f32.powi(2) * 0.25
        );
    }

    #[test]
    fn seeded_scene_shape_is_repeatable() {
        assert_eq!(FieldShape::from_seed(10), FieldShape::Square);
        assert_eq!(FieldShape::from_seed(11), FieldShape::Circle);
    }

    #[test]
    fn prime_count_curve_uses_the_exact_prime_counting_staircase() {
        let points = generate_points(5, 7);
        let curve = generate_prime_count_curve(&points, 25);
        assert_eq!(curve.exact.len(), 9);
        assert_eq!(curve.exact[0].number, 2);
        assert_eq!(curve.exact[0].count, 1);
        assert_eq!(curve.exact.last().map(|point| point.count), Some(9));
        assert_eq!(
            curve.logarithmic_integral.last().map(|point| point.number),
            Some(25)
        );
        assert!(curve.logarithmic_integral_at(25) > 9.0);
    }
}
