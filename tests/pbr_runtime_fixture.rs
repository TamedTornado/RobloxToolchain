use rbx_dom_weak::types::Variant;
use roblox_toolchain::scripts;

const PLACE: &[u8] = include_bytes!("fixtures/pbr-runtime/published-v8.rbxl");

#[test]
fn published_pbr_fixture_retains_explicit_maps_and_runtime_packs() {
    let dom = rbx_binary::from_reader(PLACE).unwrap();
    let expected = [
        ("COLOR", "88856640313512", 1),
        ("NORMAL", "120307194770369", 2),
        ("ROUGH", "79807309168451", 2),
        ("METAL", "109219752895700", 2),
        ("ALL FOUR", "114053513275441", 4),
    ];

    let surfaces: Vec<_> = dom
        .descendants()
        .filter(|node| node.class.as_str() == "SurfaceAppearance")
        .collect();
    assert_eq!(surfaces.len(), expected.len());

    for (name, pack, map_count) in expected {
        let surface = surfaces
            .iter()
            .find(|node| dom.get_by_ref(node.parent()).unwrap().name == name)
            .unwrap();
        let Variant::ContentId(actual_pack) = &surface.properties[&"TexturePack".into()] else {
            panic!("missing pack for {name}");
        };
        assert_eq!(actual_pack.as_str(), format!("rbxassetid://{pack}"));

        let actual_maps = surface
            .properties
            .iter()
            .filter(|(property, value)| {
                property.as_str().ends_with("MapContent")
                    && matches!(value, Variant::Content(content) if content.as_uri().is_some())
            })
            .count();
        assert_eq!(actual_maps, map_count, "{name}");
    }

    assert!(!dom.descendants().any(|node| {
        matches!(
            node.class.as_str(),
            "Script" | "LocalScript" | "ModuleScript"
        )
    }));
}

#[test]
fn current_lighting_fixture_does_not_require_compatibility_migration() {
    let dom = rbx_binary::from_reader(PLACE).unwrap();
    let lighting = dom
        .descendants()
        .find(|node| node.class.as_str() == "Lighting")
        .unwrap();
    let Variant::Attributes(attributes) = &lighting.properties[&"Attributes".into()] else {
        panic!("missing current lighting metadata");
    };

    assert_eq!(
        attributes.get("RBX_LightingTechnologyUnifiedMigration"),
        Some(&Variant::Bool(true))
    );
    assert_eq!(
        lighting.properties[&"LightingStyle".into()],
        Variant::Enum(rbx_dom_weak::types::Enum::from_u32(0))
    );
    assert_eq!(
        lighting.properties[&"EnvironmentSpecularScale".into()],
        Variant::Float32(1.0)
    );
}

#[test]
fn runtime_visual_harness_compiles_without_network_or_studio() {
    let source = include_str!("fixtures/pbr-runtime/play.luau");
    let config = scripts::Config {
        optimization_level: 1,
        debug_level: 1,
        type_info_level: 0,
        coverage_level: 0,
    };

    scripts::compile(source, &config).unwrap();
    for embedded in source.split("[[").skip(1) {
        scripts::compile(embedded.split("]]").next().unwrap(), &config).unwrap();
    }
}
