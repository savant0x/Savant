use anyhow::Result;
use wasmtime::component::*;
use wasmtime::{component::ResourceTable, Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};

bindgen!({
    world: "tool",
    path: "src/plugins/savant_tools.wit",
});

struct HostState {
    ctx: WasiCtx,
    table: ResourceTable,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

pub struct WasmToolHost {
    engine: Engine,
    linker: Linker<HostState>,
}

impl WasmToolHost {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        config.consume_fuel(true);

        let engine = Engine::new(&config)?;
        let mut linker = Linker::new(&engine);

        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;

        Ok(Self { engine, linker })
    }

    pub async fn execute_tool(&self, component: &Component, args: &str) -> Result<String> {
        let mut store = Store::new(
            &self.engine,
            HostState {
                ctx: WasiCtxBuilder::new()
                    .envs(&[] as &[(&str, &str)])
                    .inherit_stderr()
                    .build(),
                table: ResourceTable::new(),
            },
        );

        // ECHO Tools have a 10M fuel limit (Law #9 compliant)
        store.set_fuel(10_000_000)?;

        let tool_instance = Tool::instantiate(&mut store, component, &self.linker)?;
        let result = tool_instance
            .savant_agent_tools_tools()
            .call_execute(&mut store, args)?;

        Ok(result)
    }
}
