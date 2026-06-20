#[cfg(feature = "js_runtime")]
use std::path::Path;
#[cfg(feature = "js_runtime")]
use std::rc::Rc;
#[cfg(feature = "js_runtime")]
use deno_core::{
    ModuleLoader, ModuleSpecifier, ModuleSource, ModuleType, ModuleLoadResponse,
    ResolutionKind, JsRuntime, RuntimeOptions, RequestedModuleType
};
#[cfg(feature = "js_runtime")]
use deno_core::error::AnyError;
#[cfg(feature = "js_runtime")]
use futures_util::FutureExt;

#[cfg(feature = "js_runtime")]
pub struct TsModuleLoader;

#[cfg(feature = "js_runtime")]
impl ModuleLoader for TsModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, AnyError> {
        Ok(deno_core::resolve_import(specifier, referrer)?)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleSpecifier>,
        _is_dyn_import: bool,
        _requested_module_type: RequestedModuleType,
    ) -> ModuleLoadResponse {
        let specifier = module_specifier.clone();

        let fut = async move {
            let path = specifier.to_file_path()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid file path"))?;
            let content = std::fs::read_to_string(&path)?;

            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            let (code, module_type) = if ext == "ts" {
                let transpiled = crate::transpiler::transpile_ts_to_js(&content, &path)
                    .map_err(|e| deno_core::anyhow::anyhow!(e))?;
                (transpiled, ModuleType::JavaScript)
            } else if ext == "json" {
                (content, ModuleType::Json)
            } else {
                (content, ModuleType::JavaScript)
            };

            let source = ModuleSource::new(
                module_type,
                deno_core::ModuleSourceCode::String(code.into()),
                &specifier,
                None,
            );
            Ok(source)
        };

        ModuleLoadResponse::Async(fut.boxed_local())
    }
}

#[cfg(feature = "js_runtime")]
pub struct JsRuntimeEngine;

#[cfg(feature = "js_runtime")]
impl JsRuntimeEngine {
    pub async fn run_file(path: &Path) -> Result<(), String> {
        let specifier = ModuleSpecifier::from_file_path(path)
            .map_err(|_| format!("Invalid file path: {}", path.display()))?;

        let mut js_runtime = JsRuntime::new(RuntimeOptions {
            module_loader: Some(Rc::new(TsModuleLoader)),
            ..Default::default()
        });

        let mod_id = js_runtime.load_main_es_module(&specifier).await
            .map_err(|e| format!("Failed to load JS/TS module: {}", e))?;

        let evaluation = js_runtime.mod_evaluate(mod_id);

        js_runtime.run_event_loop(Default::default()).await
            .map_err(|e| format!("JS event loop failed: {}", e))?;

        evaluation.await.map_err(|e| format!("JS module evaluation failed: {}", e))?;

        Ok(())
    }
}

#[cfg(not(feature = "js_runtime"))]
use std::path::Path;

#[cfg(not(feature = "js_runtime"))]
pub struct JsRuntimeEngine;

#[cfg(not(feature = "js_runtime"))]
impl JsRuntimeEngine {
    pub async fn run_file(path: &Path) -> Result<(), String> {
        Err(format!(
            "JavaScript/TypeScript runtime is not enabled in this build of amae (cannot run {})",
            path.display()
        ))
    }
}
