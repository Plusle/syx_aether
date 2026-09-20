//! Shader toolchain: Slang compiler plus backend-neutral reflection.

mod compiler;
mod reflect;

pub use compiler::{CompiledShader, Compiler, ShaderConfig};
pub use reflect::{
    Binding, DescriptorType, Field, ShaderReflection, ShaderStage, ShaderStageFlags, ValueCategory,
    ValueType, ViewType,
};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::rhi::{backend::BackendType, shader::reflect};

    const FIXTURES: [&str; 12] = [
        "triangle.slang",
        "gbuffer.slang",
        "indirect_compute.slang",
        "indirect_draw.slang",
        "instance_lit.slang",
        "lighting.slang",
        "quad.slang",
        "rainbow.slang",
        "render_graph_fix.slang",
        "shadow_lit.slang",
        "shadow_map.slang",
        "simple_lit.slang",
    ];

    fn compiler() -> Compiler {
        Compiler::new(ShaderConfig::for_backend(BackendType::Vulkan)).unwrap()
    }

    fn shader_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join("shaders")
            .join(name)
    }

    fn stages_str(flags: ShaderStageFlags) -> String {
        const STAGES: [(ShaderStageFlags, &str); 6] = [
            (ShaderStageFlags::VERTEX, "VERTEX"),
            (ShaderStageFlags::HULL, "HULL"),
            (ShaderStageFlags::DOMAIN, "DOMAIN"),
            (ShaderStageFlags::GEOMETRY, "GEOMETRY"),
            (ShaderStageFlags::FRAGMENT, "FRAGMENT"),
            (ShaderStageFlags::COMPUTE, "COMPUTE"),
        ];
        let names: Vec<&str> = STAGES
            .iter()
            .filter(|(flag, _)| flags.contains(*flag))
            .map(|(_, name)| *name)
            .collect();
        if names.is_empty() {
            "-".to_string()
        } else {
            names.join("|")
        }
    }

    fn dump_compiled(name: &str, compiled: &CompiledShader) {
        println!("== {name}");
        for (entry_name, binary) in &compiled.entries {
            println!(
                "entry {entry_name} stage={:?} bytes={}",
                binary.stage,
                binary.binary.len()
            );
        }
        for binding in &compiled.reflection.bindings {
            println!(
                "binding {} set={} binding={} type={:?} count={} view={:?} padded={} R/W={}/{} stages={}",
                binding.name,
                binding.set,
                binding.binding,
                binding.ty,
                binding.count,
                binding.view_type,
                binding.padded_size,
                binding.readable,
                binding.writable,
                stages_str(binding.stages),
            );
        }
        for push in &compiled.reflection.push_constant_ranges {
            println!(
                "push stages={} offset={} size={}",
                stages_str(push.stages),
                push.offset,
                push.size
            );
        }
        for (block, fields) in &compiled.reflection.fields {
            println!("ubo {block}");
            for field in fields {
                println!(
                    "field {} offset={} size={} padded={} stride={} count={} {:?}/{:?} dim={}",
                    field.name,
                    field.offset,
                    field.size,
                    field.padded_size,
                    field.element_stride,
                    field.count,
                    field.category,
                    field.ty,
                    field.dimension,
                );
            }
        }
    }

    #[test]
    fn triangle_compiles_two_entries() {
        let compiled = compiler()
            .compile_file(&shader_path("triangle.slang"))
            .unwrap();
        let vs = compiled.entries.get("vsMain").unwrap();
        let fs = compiled.entries.get("fsMain").unwrap();
        assert_eq!(vs.stage, ShaderStage::Vertex);
        assert_eq!(fs.stage, ShaderStage::Fragment);
        assert!(!vs.binary.is_empty());
        assert!(!fs.binary.is_empty());
    }

    #[test]
    fn simple_lit_reflection() {
        let compiled = compiler()
            .compile_file(&shader_path("simple_lit.slang"))
            .unwrap();
        let reflection = &compiled.reflection;

        assert_eq!(reflection.bindings.len(), 2);
        let lighting = reflection
            .bindings
            .iter()
            .find(|b| b.name == "lighting")
            .unwrap();
        assert_eq!(lighting.set, 0);
        assert_eq!(lighting.binding, 0);
        assert_eq!(lighting.ty, DescriptorType::UniformBuffer);
        assert_eq!(lighting.padded_size, 48);
        assert!(lighting.stages.contains(ShaderStageFlags::VERTEX));
        assert!(lighting.stages.contains(ShaderStageFlags::FRAGMENT));

        let material = reflection
            .bindings
            .iter()
            .find(|b| b.name == "material")
            .unwrap();
        assert_eq!(material.set, 1);
        assert_eq!(material.binding, 0);
        assert_eq!(material.ty, DescriptorType::UniformBuffer);
        assert_eq!(material.padded_size, 16);

        assert_eq!(reflection.push_constant_ranges.len(), 1);
        let push = reflection.push_constant_ranges[0];
        assert_eq!(push.offset, 0);
        assert_eq!(push.size, 128);
        assert!(push.stages.contains(ShaderStageFlags::VERTEX));

        let lighting_fields = reflection.fields.get("lighting").unwrap();
        let field = |name: &str| {
            lighting_fields
                .iter()
                .find(|f| f.name == name)
                .unwrap()
                .clone()
        };
        let light_dir = field("lightDir");
        assert_eq!(light_dir.offset, 0);
        assert_eq!(light_dir.size, 12);
        assert_eq!(light_dir.category, ValueCategory::Vector);
        assert_eq!(light_dir.dimension, 3);
        assert_eq!(light_dir.ty, ValueType::Float);
        let light_color = field("lightColor");
        assert_eq!(light_color.offset, 16);
        assert_eq!(light_color.size, 12);
        let light_intensity = field("lightIntensity");
        assert_eq!(light_intensity.offset, 28);
        assert_eq!(light_intensity.size, 4);
        assert_eq!(light_intensity.category, ValueCategory::Scalar);

        let material_fields = reflection.fields.get("material").unwrap();
        let ambient = material_fields
            .iter()
            .find(|f| f.name == "ambient")
            .unwrap();
        assert_eq!(ambient.offset, 0);
        assert_eq!(ambient.size, 4);
        assert_eq!(ambient.ty, ValueType::Float);

        dump_compiled("simple_lit", &compiled);
    }

    #[test]
    fn merge_combines_stages_and_fields() {
        let binding = |stages: ShaderStageFlags| Binding {
            name: "tex".to_string(),
            set: 0,
            binding: 3,
            ty: DescriptorType::SampledImage,
            count: 1,
            view_type: ViewType::Dim2D,
            padded_size: 0,
            writable: false,
            readable: true,
            stages,
        };
        let field = |name: &str, offset: u32| Field {
            name: name.to_string(),
            offset,
            size: 4,
            padded_size: 4,
            element_stride: 0,
            count: 1,
            category: ValueCategory::Scalar,
            ty: ValueType::Float,
            dimension: 1,
        };
        let mut vertex = ShaderReflection::default();
        vertex.bindings.push(binding(ShaderStageFlags::VERTEX));
        vertex.fields.insert("ubo".to_string(), vec![field("a", 0)]);
        let mut fragment = ShaderReflection::default();
        fragment.bindings.push(binding(ShaderStageFlags::FRAGMENT));
        fragment
            .fields
            .insert("ubo".to_string(), vec![field("a", 0), field("b", 4)]);

        reflect::merge(&mut vertex, fragment);

        assert_eq!(vertex.bindings.len(), 1);
        let merged = &vertex.bindings[0];
        assert_eq!(merged.count, 1);
        assert!(merged.stages.contains(ShaderStageFlags::VERTEX));
        assert!(merged.stages.contains(ShaderStageFlags::FRAGMENT));
        let fields = vertex.fields.get("ubo").unwrap();
        assert_eq!(fields.len(), 2);
        assert!(fields.iter().any(|f| f.name == "b"));
    }

    #[test]
    fn all_fixtures_compile() {
        let compiler = compiler();
        for fixture in FIXTURES {
            let compiled = compiler
                .compile_file(&shader_path(fixture))
                .unwrap_or_else(|e| panic!("{fixture}: {e}"));
            assert!(!compiled.entries.is_empty(), "{fixture}: no entries");
            for (name, binary) in &compiled.entries {
                assert!(!binary.binary.is_empty(), "{fixture}/{name}: empty binary");
            }
            dump_compiled(fixture, &compiled);
        }
    }

    #[test]
    fn indirect_compute_has_compute_stage() {
        let compiled = compiler()
            .compile_file(&shader_path("indirect_compute.slang"))
            .unwrap();
        assert!(compiled
            .entries
            .values()
            .any(|b| b.stage == ShaderStage::Compute));
    }
}
