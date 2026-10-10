use nir_format::{DialogueShake, Micros, SpriteShakeMode};
use serde::{Deserialize, Serialize};

/// Per-step choices are captured once. Rendering and cold restore never draw
/// randomness, so a skipped frame cannot change the later trajectory or RNG.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShakeCapture {
    pub base: [f32; 2],
    pub targets: Vec<[i32; 2]>,
}
impl ShakeCapture {
    fn sprite_sign(mode: SpriteShakeMode, index: u64) -> i32 {
        match mode {
            SpriteShakeMode::Wave => match index % 4 {
                0 => -1,
                2 => 1,
                _ => 0,
            },
            SpriteShakeMode::Bound => {
                if index.is_multiple_of(2) {
                    -1
                } else {
                    1
                }
            }
            SpriteShakeMode::Quake => unreachable!("quake uses a shared phase capture"),
        }
    }
    pub(crate) fn sprite(
        mode: SpriteShakeMode,
        spec: DialogueShake,
        mut random: impl FnMut(u32) -> u32,
    ) -> Self {
        let targets = (0..spec.steps())
            .map(|index| {
                let sign = Self::sprite_sign(mode, index);
                spec.amplitude.map(|amplitude| {
                    let maximum = amplitude as u64
                        - index * spec.step_us.0 * amplitude as u64 / spec.duration_us.0;
                    // Source draws separately for each axis, even at zero phases.
                    let factor = if spec.randomize {
                        random(50) as u64 + 51
                    } else {
                        100
                    };
                    (maximum * factor / 100) as i32 * sign
                })
            })
            .collect();
        Self {
            base: [0.; 2],
            targets,
        }
    }
    pub(crate) fn valid_sprite(&self, mode: SpriteShakeMode, spec: DialogueShake) -> bool {
        if mode == SpriteShakeMode::Quake {
            return self.base == [0.; 2] && self.valid(spec);
        }
        spec.valid()
            && self.base == [0.; 2]
            && self.targets.len() as u64 == spec.steps()
            && self.targets.iter().enumerate().all(|(index, values)| {
                let sign = Self::sprite_sign(mode, index as u64);
                values.iter().enumerate().all(|(axis, value)| {
                    let maximum = spec.amplitude[axis] as u64
                        - index as u64 * spec.step_us.0 * spec.amplitude[axis] as u64
                            / spec.duration_us.0;
                    if spec.randomize {
                        (51..=100).any(|factor| (maximum * factor / 100) as i32 * sign == *value)
                    } else {
                        maximum as i32 * sign == *value
                    }
                })
            })
    }
    pub(crate) fn sprite_group(
        mode: SpriteShakeMode,
        spec: DialogueShake,
        nodes: &[String],
        mut random: impl FnMut(u32) -> u32,
    ) -> std::collections::BTreeMap<String, Self> {
        if mode != SpriteShakeMode::Quake {
            return nodes
                .iter()
                .map(|node| (node.clone(), Self::sprite(mode, spec, &mut random)))
                .collect();
        }
        let mut captures: std::collections::BTreeMap<_, _> = nodes
            .iter()
            .map(|node| {
                (
                    node.clone(),
                    Self {
                        base: [0.; 2],
                        targets: Vec::with_capacity(spec.steps() as usize),
                    },
                )
            })
            .collect();
        for index in 0..spec.steps() {
            // Direction is chosen before the source loop over targets.
            let signs = if index % 2 == 1 {
                [0, 0].map(|_| if random(2) == 0 { -1 } else { 1 })
            } else {
                [0, 0]
            };
            for node in nodes {
                let target = [0, 1].map(|axis| {
                    let amplitude = spec.amplitude[axis] as u64;
                    let maximum =
                        amplitude - index * spec.step_us.0 * amplitude / spec.duration_us.0;
                    let factor = if spec.randomize {
                        random(50) as u64 + 51
                    } else {
                        100
                    };
                    (maximum * factor / 100) as i32 * signs[axis]
                });
                captures.get_mut(node).unwrap().targets.push(target);
            }
        }
        captures
    }
    pub(crate) fn shared_quake_phases(captures: &std::collections::BTreeMap<String, Self>) -> bool {
        let Some(first) = captures.values().next() else {
            return false;
        };
        for index in 0..first.targets.len() {
            for axis in 0..2 {
                let mut sign = 0;
                for capture in captures.values() {
                    let Some(target) = capture.targets.get(index) else {
                        return false;
                    };
                    let current = target[axis].signum();
                    if current != 0 && sign != 0 && current != sign {
                        return false;
                    }
                    if current != 0 {
                        sign = current;
                    }
                }
            }
        }
        true
    }
    pub(crate) fn wave(spec: DialogueShake) -> Self {
        let targets = (0..spec.steps())
            .map(|index| {
                let sign = match index % 4 {
                    0 => -1,
                    2 => 1,
                    _ => 0,
                };
                spec.amplitude.map(|amplitude| {
                    let elapsed = index * spec.step_us.0;
                    let remaining =
                        amplitude as u64 - elapsed * amplitude as u64 / spec.duration_us.0;
                    remaining as i32 * sign
                })
            })
            .collect();
        Self {
            base: [0.; 2],
            targets,
        }
    }
    pub(crate) fn valid_wave(&self, spec: DialogueShake) -> bool {
        spec.valid()
            && !spec.randomize
            && self.base == [0.; 2]
            && self.targets == Self::wave(spec).targets
    }
    pub(crate) fn capture(
        spec: DialogueShake,
        base: [f32; 2],
        mut random: impl FnMut(u32) -> u32,
    ) -> Self {
        let mut targets = Vec::with_capacity(spec.steps() as usize);
        for index in 0..spec.steps() {
            let mut target = [0; 2];
            if index % 2 == 1 {
                let elapsed = index * spec.step_us.0;
                for (axis, value) in target.iter_mut().enumerate() {
                    let amplitude = spec.amplitude[axis] as u64;
                    let remaining = amplitude - elapsed * amplitude / spec.duration_us.0;
                    *value = remaining as i32 * if random(2) == 0 { -1 } else { 1 };
                }
            }
            if spec.randomize {
                for value in &mut target {
                    *value = *value * (random(50) as i32 + 51) / 100;
                }
            }
            targets.push(target);
        }
        Self { base, targets }
    }
    pub(crate) fn valid(&self, spec: DialogueShake) -> bool {
        spec.valid()
            && self.base.iter().all(|v| v.is_finite() && v.abs() <= 8192.)
            && self.targets.len() as u64 == spec.steps()
            && self.targets.iter().enumerate().all(|(index, values)| {
                values.iter().enumerate().all(|(axis, value)| {
                    let amplitude = spec.amplitude[axis] as u64;
                    let elapsed = index as u64 * spec.step_us.0;
                    let maximum = amplitude - elapsed * amplitude / spec.duration_us.0;
                    if index % 2 == 0 {
                        *value == 0
                    } else if spec.randomize {
                        (51..=100)
                            .any(|factor| maximum * factor / 100 == value.unsigned_abs() as u64)
                    } else {
                        value.unsigned_abs() as u64 == maximum
                    }
                })
            })
    }
    pub(crate) fn sample(&self, spec: DialogueShake, elapsed: Micros) -> [f32; 2] {
        if elapsed.0 >= spec.duration_us.0 {
            return [0.; 2];
        }
        let index = (elapsed.0 / spec.step_us.0) as usize;
        let t = (elapsed.0 % spec.step_us.0) / 1000;
        let duration = spec.step_us.0 / 1000;
        // The inspected stock PropMotion uses integer milliseconds, a sine
        // quarter-wave, explicit first/last samples, and truncating property
        // conversion. INC also subtracts one millisecond before its sine.
        let p = if t == 0 {
            0.
        } else if t >= duration - 1 {
            1.
        } else if index % 2 == 1 {
            1. - (((duration - t - 1) as f64 * std::f64::consts::FRAC_PI_2) / duration as f64).sin()
        } else {
            ((t as f64 * std::f64::consts::FRAC_PI_2) / duration as f64).sin()
        };
        let from = if index == 0 {
            self.base
        } else {
            self.targets[index - 1].map(|n| n as f32)
        };
        std::array::from_fn(|axis| {
            ((1. - p) * from[axis] as f64 + p * self.targets[index][axis] as f64).trunc() as f32
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sprite_quake_shares_direction_and_draws_strength_in_source_target_order() {
        let spec = DialogueShake {
            amplitude: [100, 100],
            step_us: Micros(50000),
            duration_us: Micros(200000),
            randomize: true,
        };
        let mut draws = Vec::new();
        let mut strength = 0;
        let captures = ShakeCapture::sprite_group(
            SpriteShakeMode::Quake,
            spec,
            &["z".into(), "a".into()],
            |upper| {
                draws.push(upper);
                if upper == 2 {
                    return 0;
                }
                strength += 1;
                if strength % 4 <= 1 {
                    0
                } else {
                    49
                }
            },
        );
        assert_eq!(&draws[..10], &[50, 50, 50, 50, 2, 2, 50, 50, 50, 50]);
        assert_eq!(draws.len(), 20);
        assert_ne!(captures["z"].targets, captures["a"].targets);
        assert!(captures
            .values()
            .all(|c| c.valid_sprite(SpriteShakeMode::Quake, spec)));
        assert!(ShakeCapture::shared_quake_phases(&captures));
        let mut forged = captures;
        forged.get_mut("z").unwrap().targets[1][0] *= -1;
        assert!(forged
            .values()
            .all(|c| c.valid_sprite(SpriteShakeMode::Quake, spec)));
        assert!(!ShakeCapture::shared_quake_phases(&forged));
    }
    #[test]
    fn independent_sprite_modes_keep_polarity_decay_and_axis_draw_order() {
        let mut spec = DialogueShake {
            amplitude: [10, 20],
            step_us: Micros(50000),
            duration_us: Micros(200000),
            randomize: false,
        };
        let bound = ShakeCapture::sprite(SpriteShakeMode::Bound, spec, |_| {
            panic!("deterministic mode draws no randomness")
        });
        assert_eq!(bound.targets, [[-10, -20], [8, 15], [-5, -10], [3, 5]]);
        assert!(bound.valid_sprite(SpriteShakeMode::Bound, spec));
        assert!(!bound.valid_sprite(SpriteShakeMode::Wave, spec));
        spec.randomize = true;
        let mut draws = 0;
        let wave = ShakeCapture::sprite(SpriteShakeMode::Wave, spec, |upper| {
            assert_eq!(upper, 50);
            draws += 1;
            if draws % 2 == 1 {
                0
            } else {
                49
            }
        });
        assert_eq!(draws, 8);
        assert_eq!(wave.targets, [[-5, -20], [0, 0], [2, 10], [0, 0]]);
        assert!(wave.valid_sprite(SpriteShakeMode::Wave, spec));
        let mut forged = wave;
        forged.targets[0][0] = 5;
        assert!(!forged.valid_sprite(SpriteShakeMode::Wave, spec));
    }

    #[test]
    fn wave_captures_four_phase_decay_and_rejects_altered_restore() {
        let spec = DialogueShake {
            amplitude: [0, 10],
            step_us: Micros(150_000),
            duration_us: Micros(750_000),
            randomize: false,
        };
        let capture = ShakeCapture::wave(spec);
        assert!(capture.valid_wave(spec));
        assert_eq!(capture.targets, [[0, -10], [0, 0], [0, 6], [0, 0], [0, -2]]);
        assert_eq!(capture.sample(spec, Micros(75_000)), [0., -7.]);
        assert_eq!(capture.sample(spec, Micros(149_000)), [0., -10.]);
        assert_eq!(capture.sample(spec, Micros(300_000)), [0., 0.]);
        assert_eq!(capture.sample(spec, Micros(749_000)), [0., -2.]);
        assert_eq!(capture.sample(spec, Micros(750_000)), [0.; 2]);
        let mut forged = capture;
        forged.targets[0][1] = 10;
        assert!(!forged.valid_wave(spec));
    }
    #[test]
    fn quake_captures_integer_decay_draw_order_and_stock_sine_segments() {
        let spec = DialogueShake {
            amplitude: [15, 15],
            step_us: Micros(15_000),
            duration_us: Micros(500_000),
            randomize: true,
        };
        let mut draws = vec![];
        let capture = ShakeCapture::capture(spec, [0.; 2], |upper| {
            draws.push(upper);
            upper - 1
        });
        assert!(capture.valid(spec));
        assert_eq!(&draws[..6], &[50, 50, 2, 2, 50, 50]);
        assert_eq!(capture.targets[0], [0, 0]);
        assert_eq!(capture.targets[1], [15, 15]);
        assert_eq!(capture.targets[3], [14, 14]);
        assert_eq!(capture.sample(spec, Micros(22_000)), [4., 4.]);
        assert_eq!(capture.sample(spec, Micros(29_000)), [15., 15.]);
        assert_eq!(capture.sample(spec, Micros(30_000)), [15., 15.]);
        assert_eq!(capture.sample(spec, Micros(37_000)), [4., 4.]);
        assert_eq!(capture.sample(spec, Micros(500_000)), [0.; 2]);
        let frozen: ShakeCapture =
            serde_json::from_value(serde_json::to_value(&capture).unwrap()).unwrap();
        assert_eq!(
            frozen.sample(spec, Micros(22_000)),
            capture.sample(spec, Micros(22_000))
        );
        let mut forged = frozen;
        forged.targets[0] = [1, 0];
        assert!(!forged.valid(spec));
        forged.targets.clear();
        assert!(!forged.valid(spec));
    }
}
