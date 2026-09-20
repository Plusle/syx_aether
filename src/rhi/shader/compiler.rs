//! Slang-backed shader compiler: session management, per-file compile, and
//! stage-binary collection. One `Compiler` holds the process-wide
//! `GlobalSession`; every `compile_file` call builds a fresh `Session`
//! (Slang sessions are compile-isolation units carrying search paths,
//! targets, and options). Compilation is sequential; the crate's session
//! types are not Send/Sync, so no cross-thread design is attempted.

use std::{collections::HashMap, ffi::CString, path::Path};

use shader_slang as slang;
use shader_slang::Downcast;

use crate::rhi::{
    backend::BackendType,
    error::RhiError,
    shader::reflect::{self, ShaderReflection, ShaderStage, ShaderStageFlags},
};

/// Opaque compiler configuration derived from the active backend.
#[derive(Debug)]
pub struct ShaderConfig {
    targets: Vec<slang::CompileTarget>,
    profile: String,
    emit_spirv_directly: bool,
    optimization: slang::OptimizationLevel,
    debug_info: slang::DebugInfoLevel,
}

impl ShaderConfig {
    /// Default configuration for a backend. Vulkan: SPIR-V target with the
    /// glsl_450 profile, direct SPIR-V emission (full debug info support),
    /// column-major matrices; debug builds skip optimization and emit full
    /// debug info, release builds optimize hard.
    pub fn for_backend(kind: BackendType) -> Self {
        match kind {
            BackendType::Vulkan => Self {
                targets: vec![slang::CompileTarget::Spirv],
                profile: "glsl_450".to_string(),
                emit_spirv_directly: true,
                optimization: if cfg!(debug_assertions) {
                    slang::OptimizationLevel::None
                } else {
                    slang::OptimizationLevel::High
                },
                debug_info: if cfg!(debug_assertions) {
                    slang::DebugInfoLevel::Maximal
                } else {
                    slang::DebugInfoLevel::None
                },
            },
            _ => {
                panic!("unsupported backend for shader compiler")
            }
        }
    }

    fn compiler_options(&self) -> slang::CompilerOptions {
        let mut options = slang::CompilerOptions::default()
            .matrix_layout_column(true)
            .optimization(self.optimization)
            .debug_information(self.debug_info);
        if self.emit_spirv_directly && self.targets.contains(&slang::CompileTarget::Spirv) {
            options = options.emit_spirv_directly(true);
        }
        options
    }
}

/// Binary of one entry point.
pub struct StageBinary {
    pub binary: Vec<u8>,
    pub stage: ShaderStage,
}

/// Result of compiling one shader file: per-entry-point binaries plus the
/// merged reflection.
pub struct CompiledShader {
    pub entries: HashMap<String, StageBinary>,
    pub reflection: ShaderReflection,
}

/// Shader compiler bound to one configuration.
pub struct Compiler {
    global: slang::GlobalSession,
    config: ShaderConfig,
}

fn slang_err(context: &str, err: &slang::Error) -> RhiError {
    RhiError::Shader(format!("{context}: {err}"))
}

impl Compiler {
    pub fn new(config: ShaderConfig) -> Result<Self, RhiError> {
        let global = slang::GlobalSession::new()
            .ok_or_else(|| RhiError::Shader("failed to create Slang global session".into()))?;
        Ok(Self { global, config })
    }

    /// Compiles one `.slang` file: loads it from its parent directory via
    /// the search path, links all entry points together, extracts merged
    /// reflection, and emits one binary per entry point.
    pub fn compile_file(&self, path: &Path) -> Result<CompiledShader, RhiError> {
        let parent = path
            .parent()
            .ok_or_else(|| RhiError::Shader(format!("no search path for {}", path.display())))?;
        let module_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| RhiError::Shader(format!("invalid shader path {}", path.display())))?;
        let search_path = CString::new(parent.to_string_lossy().as_bytes()).map_err(|_| {
            RhiError::Shader(format!("non-UTF8 search path for {}", path.display()))
        })?;

        let session_options = self.config.compiler_options();
        let profile = self.global.find_profile(&self.config.profile);
        let targets: Vec<slang::TargetDesc> = self
            .config
            .targets
            .iter()
            .map(|format| {
                slang::TargetDesc::default()
                    .format(*format)
                    .profile(profile)
                    .options(&session_options)
            })
            .collect();
        let search_paths = [search_path.as_ptr()];
        let session_desc = slang::SessionDesc::default()
            .targets(&targets)
            .search_paths(&search_paths)
            .options(&session_options);
        let session = self
            .global
            .create_session(&session_desc)
            .ok_or_else(|| RhiError::Shader("failed to create Slang session".into()))?;

        let module = session
            .load_module(module_name)
            .map_err(|e| slang_err(&format!("load {}", path.display()), &e))?;
        let entry_points: Vec<slang::EntryPoint> = module.entry_points().collect();
        if entry_points.is_empty() {
            return Err(RhiError::Shader(format!(
                "no entry points in {}",
                path.display()
            )));
        }

        let mut components = vec![module.downcast().clone()];
        components.extend(entry_points.iter().map(|ep| ep.downcast().clone()));
        let program = session
            .create_composite_component_type(&components)
            .map_err(|e| slang_err("create composite", &e))?;
        let linked = program.link().map_err(|e| slang_err("link", &e))?;

        let layout = linked.layout(0).map_err(|e| slang_err("reflect", &e))?;
        let mut all_stages = ShaderStageFlags::empty();
        for ep in layout.entry_points() {
            if let Some(stage) = reflect::stage_from_slang(ep.stage()) {
                all_stages |= stage.flag();
            }
        }
        let reflection = reflect::extract(layout, all_stages)?;

        let mut entries = HashMap::new();
        let refl_entry_points: Vec<&slang::reflection::EntryPoint> =
            layout.entry_points().collect();
        for (index, ep) in refl_entry_points.iter().enumerate() {
            let name = ep.name().to_string();
            let stage = reflect::stage_from_slang(ep.stage())
                .ok_or_else(|| RhiError::Shader(format!("entry point '{name}' has no stage")))?;
            let blob = linked
                .entry_point_code(index as i64, 0)
                .map_err(|e| slang_err(&format!("codegen {name}"), &e))?;
            entries.insert(
                name,
                StageBinary {
                    binary: blob.as_slice().to_vec(),
                    stage,
                },
            );
        }

        Ok(CompiledShader {
            entries,
            reflection,
        })
    }
}
