//! Ulam-spiral layout and the small, deterministic attributes used by the renderer.

#[derive(Clone, Copy, Debug)]
pub struct UlamPoint {
    /// The natural number represented by this cell. Points are stored in
    /// ascending numerical order so the renderer can reveal a prefix without
    /// simulating one cell at a time.
    pub number: u32,
    pub x: f32,
    pub y: f32,
    pub phase: f32,
    pub energy: f32,
}

/// Returns the usual counter-clockwise Ulam-spiral coordinate, with one at the origin.
pub fn spiral_coordinate(number: u32) -> (i32, i32) {
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

/// Sieve primes and place them on an odd-sized Ulam grid. A point's colour phase is
/// stable within a scene, but its seed makes the next composition feel distinct.
pub fn generate_points(side: u32, seed: u32) -> Vec<UlamPoint> {
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
        let phase = (hash as f32) * (1.0 / u32::MAX as f32);
        let normalized_age = number as f32 / largest as f32;
        // Small, deterministic variation prevents the regular grid from looking flat.
        let energy = 0.42 + 0.58 * (1.0 - normalized_age).powf(0.22);
        points.push(UlamPoint {
            number,
            x: x as f32,
            y: y as f32,
            phase,
            energy,
        });
    }
    points
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
    use super::{generate_points, spiral_coordinate};

    #[test]
    fn first_turn_of_spiral_is_in_the_expected_order() {
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
    fn five_by_five_field_contains_the_nine_primes_up_to_twenty_five() {
        assert_eq!(generate_points(5, 7).len(), 9);
    }

    #[test]
    fn even_side_is_promoted_to_an_odd_square() {
        assert_eq!(generate_points(4, 7).len(), 9);
    }
}
