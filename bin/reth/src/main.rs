#![allow(missing_docs)]
#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

use std::path::PathBuf;

use jsonrpsee::{core::client::ClientT, rpc_params};
use gwyneth::{cli::{create_gwyneth_nodes, GwynethArgs}, exex::{GwynethFullNode, L1ParentStates}};
use reth::chainspec::EthereumChainSpecParser;
use reth_node_ethereum::EthereumNode;

fn main() -> eyre::Result<()> {
    println!("WTF");
    reth::cli::Cli::<EthereumChainSpecParser, GwynethArgs>::parse_args_l2().run(|builder, arg| async move {
        println!("ignore-payload {:?}", builder.config().builder.ignore_payload);
        
        let arg = GwynethArgs {
            chain_ids: vec![167010, 167011],
            datadirs: vec![PathBuf::from("data/reth/gwyneth-167010"), PathBuf::from("data/reth/gwyneth-167011")],
            ports: Some(vec![10110, 10210]),
            ..Default::default()
        };
        
        let gwyneth_nodes = create_gwyneth_nodes(
            &arg, 
            builder.task_executor().clone(),
            builder.config()
        ).await;
        
        let l1_parents = L1ParentStates::new(&gwyneth_nodes);
        
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Rollup",   |ctx| async move {
                Ok(gwyneth::exex::Rollup::new(ctx, gwyneth_nodes, l1_parents)?.start())
            })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}


#[cfg(test)]
mod tests {
    use clap::{Args, Parser};
    
    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }
}
