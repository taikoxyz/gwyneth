#![allow(missing_docs)]
#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

use clap::{Args, Parser};
use reth::{args::{DiscoveryArgs, NetworkArgs, RpcServerArgs}, cli::Cli};
use reth_chainspec::ChainSpecBuilder;
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_node_builder::{
    engine_tree_config::{
        TreeConfig, DEFAULT_MEMORY_BLOCK_BUFFER_TARGET, DEFAULT_PERSISTENCE_THRESHOLD,
    }, EngineNodeLauncher, NodeBuilder, NodeConfig, NodeHandle
};
use reth_node_ethereum::{node::EthereumAddOns, EthereumNode};
use reth_provider::providers::BlockchainProvider2;
use reth_tasks::TaskManager;
use reth_tracing::tracing::warn;
use tracing::info;

//use reth_provider::NODES;
use gwyneth::{engine_api::RpcServerArgsExEx, GwynethNode};

// use reth::args::{DiscoveryArgs, NetworkArgs, RpcServerArgs};
// use reth_chainspec::ChainSpecBuilder;
// use reth_node_builder::{NodeBuilder, NodeConfig, NodeHandle};
// use reth_node_ethereum::EthereumNode;
// use reth_provider::NODES;
// use reth_tasks::TaskManager;

const BASE_CHAIN_ID: u64 = gwyneth::exex::BASE_CHAIN_ID; // Base chain ID for L2s
const NUM_L2_CHAINS: u64 = 2; // Number of L2 chains to create. Todo: Shall come from config */

/// Parameters for configuring the engine
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "Engine")]
pub struct EngineArgs {
    /// Enable the experimental engine features on reth binary
    ///
    /// DEPRECATED: experimental engine is default now, use --engine.legacy to enable the legacy
    /// functionality
    #[arg(long = "engine.experimental", default_value = "false")]
    pub experimental: bool,

    /// Enable the legacy engine on reth binary
    #[arg(long = "engine.legacy", default_value = "false")]
    pub legacy: bool,

    /// Configure persistence threshold for engine experimental.
    #[arg(long = "engine.persistence-threshold", conflicts_with = "legacy", default_value_t = DEFAULT_PERSISTENCE_THRESHOLD)]
    pub persistence_threshold: u64,

    /// Configure the target number of blocks to keep in memory.
    #[arg(long = "engine.memory-block-buffer-target", conflicts_with = "legacy", default_value_t = DEFAULT_MEMORY_BLOCK_BUFFER_TARGET)]
    pub memory_block_buffer_target: u64,
}

impl Default for EngineArgs {
    fn default() -> Self {
        Self {
            experimental: false,
            legacy: false,
            persistence_threshold: DEFAULT_PERSISTENCE_THRESHOLD,
            memory_block_buffer_target: DEFAULT_MEMORY_BLOCK_BUFFER_TARGET,
        }
    }
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let tasks = TaskManager::current();
        let exec = tasks.executor();
        let network_config = NetworkArgs {
            discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
            ..NetworkArgs::default()
        };

        let mut gwyneth_nodes = Vec::new();

        for i in 0..NUM_L2_CHAINS {
            let chain_id = BASE_CHAIN_ID + i; // Increment by 1 for each L2

            let chain_spec = ChainSpecBuilder::default()
                .chain(chain_id.into())
                .genesis(
                    serde_json::from_str(include_str!(
                        "../../../crates/ethereum/node/tests/assets/genesis.json"
                    ))
                    .unwrap(),
                )
                .cancun_activated()
                .build();

            let node_config = NodeConfig::test()
                .with_chain(chain_spec.clone())
                .with_network(network_config.clone())
                .with_unused_ports()
                .with_rpc(
                    RpcServerArgs::default()
                        .with_unused_ports()
                        .with_static_l2_rpc_ip_and_port(chain_id)
                );

            let chain_id = chain_spec.chain.id();

            let NodeHandle { node: gwyneth_node, node_exit_future: _ } =
                NodeBuilder::new(node_config.clone())
                    .gwyneth_node(exec.clone(), chain_id)
                    .node(GwynethNode::default())
                    .launch()
                    .await?;

            //NODES.lock().unwrap().insert(chain_id, gwyneth_node.provider.clone());
            gwyneth_nodes.push(gwyneth_node);
        }

        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Rollup", move |ctx| async {
                Ok(gwyneth::exex::Rollup::new(ctx, gwyneth_nodes).await?.start())
            })
            .launch()
            .await?;


        //NODES.lock().unwrap().insert(handle.node.chain_spec().chain.id(), handle.node.provider.clone());

        handle.wait_for_node_exit().await
    })
}


// fn main() {
//     reth_cli_util::sigsegv_handler::install();

//     if let Err(err) =
//         Cli::<EthereumChainSpecParser, EngineArgs>::parse().run(|builder, engine_args| async move {
//             if engine_args.experimental {
//                 warn!(target: "reth::cli", "Experimental engine is default now, and the --engine.experimental flag is deprecated. To enable the legacy functionality, use --engine.legacy.");
//             }

//             let tasks = TaskManager::current();
//             let exec = tasks.executor();
//             let network_config = NetworkArgs {
//                 discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
//                 ..NetworkArgs::default()
//             };

//             let mut gwyneth_nodes = Vec::new();

//             for i in 0..NUM_L2_CHAINS {
//                 let chain_id = BASE_CHAIN_ID + i; // Increment by 1 for each L2

//                 let chain_spec = ChainSpecBuilder::default()
//                     .chain(chain_id.into())
//                     .genesis(
//                         serde_json::from_str(include_str!(
//                             "../../../crates/ethereum/node/tests/assets/genesis.json"
//                         ))
//                         .unwrap(),
//                     )
//                     .cancun_activated()
//                     .build();

//                 let node_config = NodeConfig::test()
//                     .with_chain(chain_spec.clone())
//                     .with_network(network_config.clone())
//                     .with_unused_ports()
//                     .with_rpc(
//                         RpcServerArgs::default()
//                             .with_unused_ports()
//                             .with_static_l2_rpc_ip_and_port(chain_id)
//                     );

//                 let chain_id = chain_spec.chain.id();

//                 let NodeHandle { node: gwyneth_node, node_exit_future: _ } =
//                     NodeBuilder::new(node_config.clone())
//                         .gwyneth_node(exec.clone(), chain_id)
//                         .node(GwynethNode::default())
//                         .launch()
//                         .await?;

//                 //NODES.lock().unwrap().insert(chain_id, gwyneth_node.provider.clone());
//                 gwyneth_nodes.push(gwyneth_node);
//             }

//             // let handle = builder
//             //     .node(EthereumNode::default())
//             //     .install_exex("Rollup", move |ctx| async {
//             //         Ok(gwyneth::exex::Rollup::new(ctx, gwyneth_nodes).await?.start())
//             //     })
//             //     .launch()
//             //     .await?;

//             let handle = builder
//                 .node(EthereumNode::default())
//                 .install_exex("Rollup", move |ctx| async {
//                     Ok(gwyneth::exex::Rollup::new(ctx, gwyneth_nodes).await?.start())
//                 })
//                 .launch()
//                 .await?;

//             //NODES.lock().unwrap().insert(handle.node.chain_spec().chain.id(), handle.node.provider.clone());

//             handle.node_exit_future.await

//             // let use_legacy_engine = engine_args.legacy;
//             // match use_legacy_engine {
//             //     false => {
//             //         let engine_tree_config = TreeConfig::default()
//             //             .with_persistence_threshold(engine_args.persistence_threshold)
//             //             .with_memory_block_buffer_target(engine_args.memory_block_buffer_target);
//             //         let handle = builder
//             //             .with_types_and_provider::<EthereumNode, BlockchainProvider2<_>>()
//             //             .with_components(EthereumNode::components())
//             //             .with_add_ons(EthereumAddOns::default())
//             //             .launch_with_fn(|builder| {
//             //                 let launcher = EngineNodeLauncher::new(
//             //                     builder.task_executor().clone(),
//             //                     builder.config().datadir(),
//             //                     engine_tree_config,
//             //                 );
//             //                 builder.launch_with(launcher)
//             //             })
//             //             .await?;
//             //         handle.node_exit_future.await
//             //     }
//             //     true => {
//             //         info!(target: "reth::cli", "Running with legacy engine");
//             //         //let handle = builder.launch_node(EthereumNode::default()).await?;

//             //         let handle = builder
//             //             .node(EthereumNode::default())
//             //             .install_exex("Rollup", move |ctx| async {
//             //                 Ok(gwyneth::exex::Rollup::new(ctx, gwyneth_nodes).await?.start())
//             //             })
//             //             .launch()
//             //             .await?;

//             //         //NODES.lock().unwrap().insert(handle.node.chain_spec().chain.id(), handle.node.provider.clone());

//             //         handle.node_exit_future.await
//             //     }
//             // }
//         })
//     {
//         eprintln!("Error: {err:?}");
//         std::process::exit(1);
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Args, Parser};

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }
}