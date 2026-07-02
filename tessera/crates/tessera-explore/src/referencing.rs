//! Spatial-referencing geometry — mapping between a block's voxel grid and world (mm) coordinates via
//! its stored voxel→world affine. Pure math over [`tessera_core::block::array::WorldFrame`]; the
//! renderer parses user input (e.g. a `L,P,S` string) and formats output, this module just computes.

use tessera_core::block::array::WorldFrame;

/// Invert a 3×3 matrix (cofactor method), or `None` if singular.
fn inv3(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let id = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * id,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * id,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * id,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * id,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * id,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * id,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * id,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * id,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * id,
        ],
    ])
}

/// Round an affine-resolved (bounded) coordinate to an integer voxel index — truncation is intended.
#[allow(clippy::cast_possible_truncation)]
fn round_index(v: f64) -> i64 {
    v.round() as i64
}

/// Resolve a world `(L, P, S)` mm point to the nearest voxel index via the **inverse** of the stored
/// voxel→world affine (`index = R⁻¹·(world − t)`). `None` if the affine is singular.
pub fn world_to_index(wf: &WorldFrame, world: [f64; 3]) -> Option<[i64; 3]> {
    let a = &wf.affine; // row-major 3×4 [R | t]
    let r = [[a[0], a[1], a[2]], [a[4], a[5], a[6]], [a[8], a[9], a[10]]];
    let t = [a[3], a[7], a[11]];
    let inv = inv3(&r)?;
    let d = [world[0] - t[0], world[1] - t[1], world[2] - t[2]];
    let mul = |row: &[f64; 3]| row[0] * d[0] + row[1] * d[1] + row[2] * d[2];
    Some([
        round_index(mul(&inv[0])),
        round_index(mul(&inv[1])),
        round_index(mul(&inv[2])),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_to_index_inverts_the_affine() {
        // 2 mm isotropic voxels, LPS, with a translation — a diagonal affine.
        let wf = WorldFrame {
            affine: [
                2.0, 0.0, 0.0, -100.0, //
                0.0, 2.0, 0.0, -50.0, //
                0.0, 0.0, 2.0, 10.0,
            ],
            convention: "LPS".into(),
            unit: "mm".into(),
            space: "scanner".into(),
        };
        // world (0,0,10) → index ((0+100)/2, (0+50)/2, (10-10)/2) = (50, 25, 0).
        assert_eq!(world_to_index(&wf, [0.0, 0.0, 10.0]), Some([50, 25, 0]));
        // A point that rounds: (-98,-48,12) → (1, 1, 1).
        assert_eq!(world_to_index(&wf, [-98.0, -48.0, 12.0]), Some([1, 1, 1]));
        // Singular affine → None.
        let sing = WorldFrame {
            affine: [0.0; 12],
            ..wf
        };
        assert_eq!(world_to_index(&sing, [1.0, 2.0, 3.0]), None);
    }
}
