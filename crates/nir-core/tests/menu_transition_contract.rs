//! P2.4 ui.menu-transition.v1: a spatial menu-page reveal style is gated by
//! its own capability, dissolve and unstyled boundaries stay on the legacy
//! alpha fade, masks must be image assets inside the theme closure, and the
//! reveal window keeps its authored bounds.
use nir_core::*;
use nir_format::*;
use serde_json::json;

fn program(effects: serde_json::Value, capabilities: &[&str]) -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires
        .extend(capabilities.iter().map(|c| (*c).to_string()));
    let menu: ImageMenu = serde_json::from_value(json!({
        "background": "bg.station", "buttons": [], "effects": effects
    }))
    .unwrap();
    p.theme.image_menus.insert("title".into(), menu);
    p
}

fn wipe_effects() -> serde_json::Value {
    json!({"enter": {"fade_us": "400000",
        "style": {"type": "wipe", "direction": "left_to_right", "softness": 0.2}}})
}

#[test]
fn spatial_styles_are_gated_by_their_own_capability() {
    // The effects capability alone does not admit a spatial reveal.
    assert_eq!(
        ValidatedProgram::new(program(wipe_effects(), &["ui.menu-effects.v1"]))
            .unwrap_err()
            .code,
        "E_CAPABILITY"
    );
    let validated = ValidatedProgram::new(program(
        wipe_effects(),
        &["ui.menu-effects.v1", "ui.menu-transition.v1"],
    ))
    .expect("both capabilities admit the spatial reveal");
    assert!(
        validated
            .program()
            .theme
            .image_menus["title"]
            .effects
            .as_ref()
            .unwrap()
            .uses_transition()
    );
}

#[test]
fn dissolve_and_unstyled_boundaries_need_no_reveal_capability() {
    for effects in [
        json!({"enter": {"sound": "audio.bell", "fade_us": "400000"}}),
        json!({"enter": {"fade_us": "400000", "style": {"type": "dissolve"}}}),
    ] {
        ValidatedProgram::new(program(effects, &["ui.menu-effects.v1"]))
            .unwrap_or_else(|e| panic!("legacy fade must validate: {e:?}"));
    }
    // A dissolve-only theme never claims the reveal capability by usage, and
    // an unused declaration stays legal — the compiler trims it instead.
    let mut p = program(
        json!({"enter": {"fade_us": "400000", "style": {"type": "dissolve"}}}),
        &["ui.menu-effects.v1", "ui.menu-transition.v1"],
    );
    assert!(!p.theme.image_menus["title"]
        .effects
        .as_ref()
        .unwrap()
        .uses_transition());
    ValidatedProgram::new(p).unwrap();
}

#[test]
fn the_reveal_window_keeps_its_authored_bounds() {
    for (fade_us, why) in [("0", "a styled boundary cannot be instant"), ("2000001", "over the two-second page ceiling")] {
        let effects = json!({"enter": {"fade_us": fade_us,
            "style": {"type": "wipe", "direction": "left_to_right"}}});
        assert_eq!(
            ValidatedProgram::new(program(
                effects,
                &["ui.menu-effects.v1", "ui.menu-transition.v1"]
            ))
            .unwrap_err()
            .code,
            "E_VIEW_EFFECTS",
            "{why}"
        );
    }
    // A sound-only boundary stays instant and unstyled.
    ValidatedProgram::new(program(
        json!({"enter": {"sound": "audio.bell"}}),
        &["ui.menu-effects.v1"],
    ))
    .unwrap();
}

#[test]
fn mask_styles_name_image_assets_inside_the_theme_closure() {
    let mask = |asset: &str| {
        json!({"enter": {"fade_us": "400000",
            "style": {"type": "mask", "asset": asset, "channel": "alpha"}}})
    };
    // An audio asset is not a page image.
    assert_eq!(
        ValidatedProgram::new(program(
            mask("audio.bgm"),
            &["ui.menu-effects.v1", "ui.menu-transition.v1"]
        ))
        .unwrap_err()
        .code,
        "E_THEME_ASSET"
    );
    ValidatedProgram::new(program(
        mask("bg.station"),
        &["ui.menu-effects.v1", "ui.menu-transition.v1"],
    ))
    .expect("an image mask joins the theme closure");
}
