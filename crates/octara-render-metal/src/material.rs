use octara_model::paint::PaintProgram;
use octara_render::plan::Material;

/// An MSL function `float4 material(float2 uv, float4 params)` for an element's rounded box.
/// Return straight linear-light RGBA. The renderer provides coverage and clipping.
pub fn material_msl(source: &'static str) -> PaintProgram {
    PaintProgram::new(Material::new_msl(source))
}

#[cfg(feature = "custom-wgsl")]
pub(crate) fn translate(source: &str) -> Result<String, String> {
    use naga::back::msl;
    use naga::valid::{Capabilities, ValidationFlags, Validator};

    let module = naga::front::wgsl::parse_str(source).map_err(|error| error.emit_to_string(source))?;
    if !module.entry_points.is_empty()
        || !module.global_variables.is_empty()
        || !module.functions.iter().any(|(_, function)| function.name.as_deref() == Some("material"))
    {
        return Err("Expected a WGSL material function without entry points or global resources".into());
    }
    let info = Validator::new(ValidationFlags::all(), Capabilities::empty()).validate(&module).map_err(|error| error.to_string())?;
    let options = msl::Options { lang_version: (2, 3), ..Default::default() };
    msl::write_string(&module, &info, &options, &msl::PipelineOptions::default())
        .map(|(source, _)| source)
        .map_err(|error| error.to_string())
}

#[cfg(all(test, feature = "custom-wgsl"))]
mod tests {
    use super::translate;

    #[test]
    fn exports_the_material_callable() {
        let translated = translate("fn material(uv: vec2f, params: vec4f) -> vec4f { return vec4f(uv, params.xy); }").unwrap();
        assert!(translated.contains("material("), "{translated}");
    }
}
