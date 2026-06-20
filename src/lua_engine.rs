use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use mlua::{Lua, Table, Value, Function};

#[derive(Debug, Clone, Default)]
pub struct LuaConfig {
    pub registry: Option<String>,
    pub store_dir: Option<PathBuf>,
}

pub struct LuaEngine {
    lua: Lua,
    preinstall_hook: Arc<Mutex<Option<mlua::RegistryKey>>>,
    postinstall_hook: Arc<Mutex<Option<mlua::RegistryKey>>>,
}

impl LuaEngine {
    pub fn new() -> Result<Self, String> {
        let lua = Lua::new();

        let preinstall_hook = Arc::new(Mutex::new(None));
        let postinstall_hook = Arc::new(Mutex::new(None));

        let engine = Self {
            lua,
            preinstall_hook,
            postinstall_hook,
        };

        engine.setup_globals()?;
        Ok(engine)
    }

    fn setup_globals(&self) -> Result<(), String> {
        let globals = self.lua.globals();

        let amae_module = self.lua.create_table().map_err(|e| e.to_string())?;

        let log_fn = self.lua.create_function(|_, msg: String| {
            println!("[Lua] {}", msg);
            Ok(())
        }).map_err(|e| e.to_string())?;
        amae_module.set("log", log_fn).map_err(|e| e.to_string())?;

        let exists_fn = self.lua.create_function(|_, path: String| {
            Ok(Path::new(&path).exists())
        }).map_err(|e| e.to_string())?;
        amae_module.set("fs_exists", exists_fn).map_err(|e| e.to_string())?;

        globals.set("amae", amae_module.clone()).map_err(|e| e.to_string())?;

        if let Ok(package) = globals.get::<_, Table>("package") {
            if let Ok(loaded) = package.get::<_, Table>("loaded") {
                let _ = loaded.set("amae", amae_module);
            }
        }

        Ok(())
    }

    pub fn load_config(&self, project_dir: &Path) -> Result<LuaConfig, String> {
        let config_path = project_dir.join("amae.config.lua");
        if !config_path.exists() {
            return Ok(LuaConfig::default());
        }

        let config_content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read amae.config.lua: {}", e))?;

        let value: Value = self.lua.load(&config_content)
            .eval()
            .map_err(|e| format!("Failed to execute amae.config.lua: {}", e))?;

        let mut config = LuaConfig::default();

        if let Value::Table(table) = value {
            if let Ok(reg) = table.get::<_, String>("registry") {
                config.registry = Some(reg);
            }

            if let Ok(store) = table.get::<_, String>("store_dir") {
                config.store_dir = Some(PathBuf::from(store));
            }

            if let Ok(hooks_table) = table.get::<_, Table>("hooks") {
                if let Ok(pre_fn) = hooks_table.get::<_, Function>("preinstall") {
                    let key = self.lua.create_registry_value(pre_fn).map_err(|e| e.to_string())?;
                    let mut lock = self.preinstall_hook.lock().unwrap();
                    *lock = Some(key);
                }
                if let Ok(post_fn) = hooks_table.get::<_, Function>("postinstall") {
                    let key = self.lua.create_registry_value(post_fn).map_err(|e| e.to_string())?;
                    let mut lock = self.postinstall_hook.lock().unwrap();
                    *lock = Some(key);
                }
            }
        }

        Ok(config)
    }

    pub fn run_preinstall_hook(&self) -> Result<(), String> {
        let lock = self.preinstall_hook.lock().unwrap();
        if let Some(ref key) = *lock {
            let func: Function = self.lua.registry_value(key).map_err(|e| e.to_string())?;
            func.call::<_, ()>(()).map_err(|e| format!("Lua preinstall hook failed: {}", e))?;
        }
        Ok(())
    }

    pub fn run_postinstall_hook(&self) -> Result<(), String> {
        let lock = self.postinstall_hook.lock().unwrap();
        if let Some(ref key) = *lock {
            let func: Function = self.lua.registry_value(key).map_err(|e| e.to_string())?;
            func.call::<_, ()>(()).map_err(|e| format!("Lua postinstall hook failed: {}", e))?;
        }
        Ok(())
    }
}
