// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use wasmtime::component::*;
use wasmtime::{component::ResourceTable, Config, Engine, Store, StoreContextMut};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};

bindgen!({
    world: "plugin",
    path: "src/plugins/savant_hooks.wit",
});

use exports::savant::agent_hooks::hooks::HookResult;

use pqcrypto_dilithium::dilithium2;
use savant_core::traits::Tool;
use savant_security::{AgentToken, SecurityAuthority};

struct HostState {
    ctx: WasiCtx,
    table: ResourceTable,
    enclave: Arc<SecurityAuthority>,
    agent_id: u64,
    token: Option<AgentToken>,
    tool_registry: HashMap<String, Arc<dyn Tool>>,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

pub struct WasmPluginHost {
    engine: Engine,
    linker: Linker<HostState>,
    enclave: Arc<SecurityAuthority>,
    tool_registry: HashMap<String, Arc<dyn Tool>>,
}

impl WasmPluginHost {
    pub fn new(
        root_authority: ed25519_dalek::VerifyingKey,
        pqc_authority: Option<dilithium2::PublicKey>,
        tool_registry: HashMap<String, Arc<dyn Tool>>,
    ) -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        config.consume_fuel(true);

        let engine = Engine::new(&config)?;
        let mut linker = Linker::new(&engine);
        let enclave = Arc::new(SecurityAuthority::new(root_authority, pqc_authority));

        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;

        // Implement the host interface with CCT verification and real tool dispatch
        linker.instance("host")?.func_wrap_async(
            "call-tool",
            move |mut cx: StoreContextMut<'_, HostState>, (tool_name, args): (String, String)| {
                Box::new(async move {
                    let state = cx.data_mut();
                    let token = state
                        .token
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("No security token provided"))?;

                    // CRITICAL: Stateless cryptographic verification
                    state
                        .enclave
                        .verify_token_and_action(token, state.agent_id, &tool_name, "execute")
                        .map_err(|e| anyhow::anyhow!("Security Boundary Violation: {}", e))?;

                    // Real tool dispatch via registry
                    if let Some(tool) = state.tool_registry.get(&tool_name) {
                        let payload = serde_json::from_str::<serde_json::Value>(&args)
                            .unwrap_or_else(|_| serde_json::json!({"payload": args}));
                        let result = tool
                            .execute(payload)
                            .await
                            .map_err(|e| anyhow::anyhow!("Tool execution failed: {}", e))?;
                        Ok((result,))
                    } else {
                        Err(anyhow::anyhow!("Tool not found in registry: {}", tool_name))
                    }
                })
            },
        )?;

        Ok(Self {
            engine,
            linker,
            enclave,
            tool_registry,
        })
    }

    pub async fn load_plugin(&self, path: impl AsRef<Path>) -> Result<Component> {
        Component::from_file(&self.engine, path)
    }

    pub async fn execute_before_llm_call(
        &self,
        component: &Component,
        prompt: &str,
        agent_id: u64,
        token: Option<AgentToken>,
    ) -> Result<HookResult> {
        let mut store = Store::new(
            &self.engine,
            HostState {
                ctx: WasiCtxBuilder::new()
                    .envs(&[] as &[(&str, &str)])
                    .inherit_stdout()
                    .build(),
                table: ResourceTable::new(),
                enclave: self.enclave.clone(),
                agent_id,
                token,
                tool_registry: self.tool_registry.clone(),
            },
        );
        store.set_fuel(1_000_000)?;

        let plugin = Plugin::instantiate(&mut store, component, &self.linker)?;
        let res = plugin
            .savant_agent_hooks_hooks()
            .call_before_llm_call(&mut store, prompt)?;

        Ok(res)
    }

    pub async fn execute_after_tool_call(
        &self,
        component: &Component,
        tool_name: &str,
        result: &str,
        agent_id: u64,
        token: Option<AgentToken>,
    ) -> Result<HookResult> {
        let mut store = Store::new(
            &self.engine,
            HostState {
                ctx: WasiCtxBuilder::new().inherit_stdout().build(),
                table: ResourceTable::new(),
                enclave: self.enclave.clone(),
                agent_id,
                token,
                tool_registry: self.tool_registry.clone(),
            },
        );
        store.set_fuel(1_000_000)?;

        let plugin = Plugin::instantiate(&mut store, component, &self.linker)?;
        let res = plugin
            .savant_agent_hooks_hooks()
            .call_after_tool_call(&mut store, tool_name, result)?;

        Ok(res)
    }

    pub async fn execute_before_response_emit(
        &self,
        component: &Component,
        response: &str,
        agent_id: u64,
        token: Option<AgentToken>,
    ) -> Result<HookResult> {
        let mut store = Store::new(
            &self.engine,
            HostState {
                ctx: WasiCtxBuilder::new()
                    .envs(&[] as &[(&str, &str)])
                    .inherit_stdout()
                    .build(),
                table: ResourceTable::new(),
                enclave: self.enclave.clone(),
                agent_id,
                token,
                tool_registry: self.tool_registry.clone(),
            },
        );
        store.set_fuel(1_000_000)?;

        let plugin = Plugin::instantiate(&mut store, component, &self.linker)?;
        let res = plugin
            .savant_agent_hooks_hooks()
            .call_before_response_emit(&mut store, response)?;

        Ok(res)
    }
}
