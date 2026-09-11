use anyhow::Result;
use bitcoin::Network;
use futures::future::BoxFuture;
use serde_json::Value;
use std::{collections::HashMap, path::Path, sync::Arc};
use tarpc::context;
use tokio::sync::RwLock;

use crate::alkanes::trace::EspoBlock;
use crate::config::get_module_config;
use crate::runtime::mdb::Mdb;
use rocksdb::{DB, Options};

/// Object-safe handler: (Context, JSON) -> JSON (async)
type HandlerFn = dyn Fn(context::Context, Value) -> BoxFuture<'static, Value> + Send + Sync;

#[derive(Clone, Default)]
pub struct RpcRegistry {
    inner: Arc<RwLock<HashMap<String, Arc<HandlerFn>>>>,
}

impl RpcRegistry {
    pub async fn register<F, Fut>(&self, name: impl Into<String>, f: F)
    where
        F: Fn(context::Context, Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Value> + Send + 'static,
    {
        let name = name.into();
        let arc: Arc<HandlerFn> = Arc::new(move |cx, val| Box::pin(f(cx, val)));
        self.inner.write().await.insert(name, arc);
    }

    pub async fn list(&self) -> Vec<String> {
        self.inner.read().await.keys().cloned().collect()
    }

    pub async fn call(&self, cx: context::Context, method: &str, payload: Value) -> Value {
        match self.inner.read().await.get(method) {
            Some(h) => h(cx, payload).await,
            None => serde_json::json!({ "error": format!("unknown method: {method}") }),
        }
    }
}

/// A namespaced registrar that forces a fixed prefix on all registered RPC methods.
/// Modules only receive this, so they cannot choose global names.
/// For module "ammdata", every method becomes "ammdata.<suffix>".
#[derive(Clone)]
pub struct RpcNsRegistrar {
    inner: RpcRegistry,
    prefix: String, // e.g., "ammdata."
}

pub struct RpcCtx {
    pub registrar: RpcNsRegistrar,
    pub mdb: Mdb,
}

impl RpcNsRegistrar {
    pub fn new(inner: RpcRegistry, module_name: &str) -> Self {
        // Always end with a dot
        let mut prefix = String::with_capacity(module_name.len() + 1);
        prefix.push_str(module_name);
        if !prefix.ends_with('.') {
            prefix.push('.');
        }
        Self { inner, prefix }
    }

    /// Register a method using only the suffix; full name will be "<prefix><suffix>".
    pub async fn register<F, Fut>(&self, suffix: impl Into<String>, f: F)
    where
        F: Fn(context::Context, Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Value> + Send + 'static,
    {
        // Normalize suffix (strip any accidental leading dot)
        let mut s = suffix.into();
        if let Some(stripped) = s.strip_prefix('.') {
            s = stripped.to_string();
        }
        let full = format!("{}{}", self.prefix, s);
        self.inner.register(full, f).await;
    }

    /// Optional: list only methods under this namespace (helper).
    pub async fn list(&self) -> Vec<String> {
        self.inner
            .list()
            .await
            .into_iter()
            .filter(|m| m.starts_with(&self.prefix))
            .collect()
    }
}

/// Object-safe module interface (storable as dyn)
pub trait EspoModule: Send + Sync {
    fn get_name(&self) -> &'static str;

    /// Injected by the registry before boxing into Arc<dyn EspoModule>
    fn set_mdb(&mut self, mdb: Arc<Mdb>);

    fn get_genesis_block(&self, network: Network) -> u32;

    fn index_block(&self, block: EspoBlock) -> Result<()>;
    fn get_index_height(&self) -> Option<u32>;

    /// Optional pre-reorg check, run on *every* module before anything is
    /// mutated. A module that knows it cannot roll back to `next_height`
    /// should fail here, while the index is still consistent.
    ///
    /// This one keeps a no-op default on purpose: it is a *veto*, and a module
    /// with nothing to veto is correct to say nothing. Contrast
    /// [`EspoModule::handle_reorg`] below, where silence is a bug.
    fn preflight_reorg(&self, _next_height: u32) -> Result<()> {
        Ok(())
    }

    /// Roll this module's in-memory state back so it agrees with storage as of
    /// `next_height - 1`. Called by
    /// [`crate::modules::reorg::handle_reorg_switch`] *after* the shared
    /// versioned tree has already been rewound.
    ///
    /// For almost every module the whole body is three lines: re-read the
    /// persisted index height and overwrite the cached copy. Modules do **not**
    /// delete their own rows — `rewind_tree_to_before` already did that for the
    /// entire versioned namespace.
    ///
    /// # Why this has no default implementation
    ///
    /// It used to. The default was `Ok(())`, and on 2026-09-11 that cost us a
    /// 36-minute production outage: `explorerextensions` caches its index
    /// height in an `RwLock` and never implemented `handle_reorg`, so on a
    /// reorg at height 966500 every other module rolled back and
    /// `explorerextensions` kept reporting the pre-reorg height. The strict
    /// verification pass in `handle_reorg_switch` — correctly — refused to
    /// keep indexing, the indexer stopped for good, and the pod stayed
    /// `Running` but unready with a climbing heap until a human restarted it.
    ///
    /// A silent no-op default plus a fatal verification pass is a trap: the
    /// module compiles, passes review, indexes happily for weeks, and then
    /// takes production down the first time the chain reorgs. Nothing catches
    /// it in between, because you cannot detect "this module forgot to refresh
    /// its cache" at registration time — a no-op `handle_reorg` and a correct
    /// one are indistinguishable until there is actually something to roll
    /// back.
    ///
    /// So the check moved to the only place that *can* catch it before a
    /// reorg does: the compiler. Every module must now say, explicitly, what
    /// it does on a reorg. A module that genuinely has no rollback state still
    /// works — it writes `Ok(())` with a comment saying why, which is a claim
    /// a reviewer can check, rather than an omission nobody can see.
    ///
    /// ```
    /// # use anyhow::Result;
    /// # use bitcoin::Network;
    /// # use espo::alkanes::trace::EspoBlock;
    /// # use espo::modules::defs::{EspoModule, RpcNsRegistrar};
    /// # use espo::runtime::mdb::Mdb;
    /// # use std::sync::Arc;
    /// struct Stateless;
    ///
    /// impl EspoModule for Stateless {
    ///     fn get_name(&self) -> &'static str { "stateless" }
    ///     fn set_mdb(&mut self, _mdb: Arc<Mdb>) {}
    ///     fn get_genesis_block(&self, _n: Network) -> u32 { 0 }
    ///     fn index_block(&self, _block: EspoBlock) -> Result<()> { Ok(()) }
    ///     fn get_index_height(&self) -> Option<u32> { None }
    ///     // Holds no persisted state and caches no height, so there is
    ///     // nothing to roll back. Explicit, and therefore reviewable.
    ///     fn handle_reorg(&self, _next_height: u32) -> Result<()> { Ok(()) }
    ///     fn register_rpc(&self, _reg: &RpcNsRegistrar) {}
    /// }
    /// ```
    ///
    /// Leaving it out no longer compiles — this is the same impl with
    /// `handle_reorg` deleted:
    ///
    /// ```compile_fail
    /// # use anyhow::Result;
    /// # use bitcoin::Network;
    /// # use espo::alkanes::trace::EspoBlock;
    /// # use espo::modules::defs::{EspoModule, RpcNsRegistrar};
    /// # use espo::runtime::mdb::Mdb;
    /// # use std::sync::Arc;
    /// struct Forgetful;
    ///
    /// impl EspoModule for Forgetful {
    ///     fn get_name(&self) -> &'static str { "forgetful" }
    ///     fn set_mdb(&mut self, _mdb: Arc<Mdb>) {}
    ///     fn get_genesis_block(&self, _n: Network) -> u32 { 0 }
    ///     fn index_block(&self, _block: EspoBlock) -> Result<()> { Ok(()) }
    ///     fn get_index_height(&self) -> Option<u32> { Some(0) }
    ///     fn register_rpc(&self, _reg: &RpcNsRegistrar) {}
    /// }
    /// ```
    fn handle_reorg(&self, next_height: u32) -> Result<()>;

    /// Modules can only register RPCs via a namespaced registrar.
    /// For a module named "ammdata", all methods will be "ammdata.<suffix>".
    fn register_rpc(&self, reg: &RpcNsRegistrar);

    /// Return Some to declare this module expects a config section.
    /// The string should describe the expected shape (used for error messages).
    fn config_spec(&self) -> Option<&'static str> {
        None
    }

    /// Load the module config section from config.json.
    fn set_config(&mut self, _config: &serde_json::Value) -> Result<()> {
        Ok(())
    }
}

/// Registry that holds modules, the RPC router, and one shared RocksDB
pub struct ModuleRegistry {
    modules: Vec<Arc<dyn EspoModule>>,
    pub router: RpcRegistry,
    module_db: Option<Arc<DB>>,
}

impl ModuleRegistry {
    /// Construct from an existing Arc<DB> (one global DB shared by all modules).
    pub fn with_db(module_db: Arc<DB>) -> Self {
        Self { modules: Vec::new(), router: RpcRegistry::default(), module_db: Some(module_db) }
    }

    /// Client mode: no database exists; every module runs on the null Mdb
    /// backend and serves its getters from the remote espo.
    pub fn with_null_backend() -> Self {
        Self { modules: Vec::new(), router: RpcRegistry::default(), module_db: None }
    }

    /// Convenience: open a global read-write DB at a path, create if missing.
    pub fn with_db_path(path: impl AsRef<Path>) -> Result<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let db = Arc::new(DB::open(&opts, path)?);
        Ok(Self::with_db(db))
    }

    /// Convenience: open a global read-only DB at a path.
    pub fn with_db_path_read_only(
        path: impl AsRef<Path>,
        error_if_log_file_exist: bool,
    ) -> Result<Self> {
        let opts = Options::default();
        let db = Arc::new(DB::open_for_read_only(&opts, path, error_if_log_file_exist)?);
        Ok(Self::with_db(db))
    }

    /// Register a module:
    /// - build a namespaced `Mdb` from the shared DB using `get_name()` as the prefix,
    /// - inject it via `set_mdb`,
    /// - provide a namespaced RPC registrar so the module can only register under "<name>.*".
    pub fn register_module<M>(&mut self, mut module: M)
    where
        M: EspoModule + 'static,
    {
        let name = module.get_name();
        if self.modules.is_empty() {
            if name != "essentials" {
                panic!("ModuleRegistry requires essentials to be registered first (got {name})");
            }
        } else if name == "essentials" {
            panic!("ModuleRegistry requires essentials to be registered before any other modules");
        } else if self.modules.first().map(|m| m.get_name() != "essentials").unwrap_or(true) {
            panic!("ModuleRegistry requires essentials to be registered first");
        }

        if let Some(spec) = module.config_spec() {
            let cfg = get_module_config(name).unwrap_or_else(|| {
                panic!(
                    "No config defined for {name} module, but {name} module was loaded and defines a config. Expected: {spec}"
                )
            });
            if let Err(e) = module.set_config(cfg) {
                panic!("module '{name}' config invalid; expected: {spec}; error: {e:?}");
            }
        }

        // --- Mdb prefix like "ammdata:" ---
        let mut prefix_kv = Vec::with_capacity(name.len() + 1);
        prefix_kv.extend_from_slice(name.as_bytes());
        prefix_kv.push(b':');

        let mdb = Arc::new(match &self.module_db {
            Some(db) => Mdb::from_db(Arc::clone(db), prefix_kv),
            None => Mdb::null(prefix_kv),
        });
        module.set_mdb(mdb);

        // --- RPC prefix like "ammdata." ---
        let ns = RpcNsRegistrar::new(self.router.clone(), name);

        let m = Arc::new(module);
        m.register_rpc(&ns);

        self.modules.push(m);
    }

    pub fn modules(&self) -> &[Arc<dyn EspoModule>] {
        &self.modules
    }
}
