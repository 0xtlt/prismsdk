//! A family's font files (bundled or loaded at run time): which one draws
//! a weight and a style. XAML names a font by its file
//! (`ms-appx:///…/Inter-Bold.ttf#Inter`) and synthesizes the weights and
//! slants that file lacks, so each text takes the file closest to its
//! font, as CSS matches faces. No Win32: tested on every host.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

/// One file of a family.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Face {
    /// Its weight (the default instance's, for a variable font).
    pub weight: u16,
    /// A variable font with a weight axis: every weight.
    pub variable: bool,
    pub italic: bool,
    /// What XAML names it by.
    pub source: String,
}

/// Adds `face` to `faces`, replacing the one of the same weight and style
/// (a newer file of it).
pub fn add(faces: &mut Vec<Face>, face: Face) {
    match faces
        .iter_mut()
        .find(|f| (f.weight, f.variable, f.italic) == (face.weight, face.variable, face.italic))
    {
        Some(f) => *f = face,
        None => faces.push(face),
    }
}

/// The face that draws `weight` (CSS, 1–1000) `italic`: the style first,
/// then the weight as CSS picks it (for 400–500 the weights up to 500,
/// then lighter, then bolder; lighter first below 400, bolder first above
/// 500). A variable font covers its weights.
pub fn pick(faces: &[Face], weight: u16, italic: bool) -> Option<&Face> {
    faces.iter().min_by_key(|f| {
        let style = u32::from(f.italic != italic);
        (style, distance(f, weight))
    })
}

/// How far a face is from `want` in CSS's order of preference (0: exact).
fn distance(f: &Face, want: u16) -> u32 {
    if f.variable {
        return 0;
    }
    let (have, want) = (u32::from(f.weight), u32::from(want));
    // Ranks: the preferred direction (0…999), then the other (1000…).
    match want {
        400..=500 if (want..=500).contains(&have) => have - want,
        400..=500 if have < want => 1000 + want - have,
        400..=500 => 2000 + have - want,
        _ if want < 400 && have <= want => want - have,
        _ if want < 400 => 1000 + have - want,
        _ if have >= want => have - want,
        _ => 1000 + want - have,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(weight: u16, italic: bool) -> Face {
        Face {
            weight,
            variable: false,
            italic,
            source: format!("{weight}{}", if italic { "i" } else { "" }),
        }
    }

    fn picked(faces: &[Face], weight: u16, italic: bool) -> &str {
        &pick(faces, weight, italic).unwrap().source
    }

    #[test]
    fn the_closest_file_draws_each_weight() {
        let faces = [face(300, false), face(400, false), face(700, false)];
        assert_eq!(picked(&faces, 400, false), "400");
        assert_eq!(picked(&faces, 700, false), "700");
        // Bolder first above 500, lighter first below 400.
        assert_eq!(picked(&faces, 600, false), "700");
        assert_eq!(picked(&faces, 900, false), "700");
        assert_eq!(picked(&faces, 200, false), "300");
        assert_eq!(picked(&faces, 350, false), "300");
        // 500 takes 400 when there is no 500 (lighter before bolder).
        assert_eq!(picked(&faces, 500, false), "400");
        assert_eq!(pick(&[], 400, false), None);
    }

    #[test]
    fn the_style_comes_before_the_weight() {
        let faces = [face(400, false), face(700, false), face(400, true)];
        assert_eq!(picked(&faces, 700, true), "400i");
        assert_eq!(picked(&faces, 700, false), "700");
        // No italic file: the upright one, slanted by XAML.
        assert_eq!(picked(&faces[..2], 400, true), "400");
    }

    #[test]
    fn a_variable_font_covers_every_weight() {
        let mut faces = vec![face(700, false)];
        add(
            &mut faces,
            Face {
                variable: true,
                ..face(400, false)
            },
        );
        assert_eq!(pick(&faces, 300, false).unwrap().weight, 400);
    }

    #[test]
    fn a_newer_file_of_a_face_replaces_it() {
        let mut faces = vec![face(400, false)];
        add(
            &mut faces,
            Face {
                source: "new".into(),
                ..face(400, false)
            },
        );
        add(&mut faces, face(700, false));
        assert_eq!(faces.len(), 2);
        assert_eq!(faces[0].source, "new");
    }
}
