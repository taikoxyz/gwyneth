use std::{collections::HashMap, convert::Infallible, path::PathBuf, sync::Arc};

use crate::exex::GwynethFullNode1;
use reth_chain_state::CanonStateSubscriptions;
use reth_db::DatabaseEnv;
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_network::NetworkHandle;
use reth_node_builder::PayloadBuilderConfig;
use reth_node_builder::{
    components::{ComponentsBuilder, PayloadServiceBuilder},
    rpc::{EngineValidatorBuilder, RpcAddOns},
    BuilderContext, LaunchNode, Node, NodeAdapter,
    NodeBuilder, NodeComponentsBuilder,
};
use reth_primitives::ChainDA;
use reth_primitives::{transaction::WithEncoded, EthPrimitives, TransactionSigned};
use reth_provider::{
    StateProviderBox, StateProviderFactory,
};
use reth_rpc::EthApi;
use reth_tasks::TaskManager;
use reth_transaction_pool::TransactionPool;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use alloy_eips::eip4895::Withdrawals;
use alloy_genesis::Genesis;
use alloy_primitives::{Address, ChainId, B256};
use alloy_rpc_types::{
    engine::{
        ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4,
        ExecutionPayloadV1, PayloadAttributes as EthPayloadAttributes, PayloadId,
    },
    Withdrawal,
};
use alloy_sol_types::sol;

use builder::default_gwyneth_payload;
use reth_basic_payload_builder::{
    BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig, BuildArguments, BuildOutcome,
    PayloadBuilder, PayloadConfig,
};
use reth_chainspec::{Chain, ChainSpec, ChainSpecProvider, EthereumHardforks};
use reth_node_api::{
    payload::{EngineApiMessageVersion, EngineObjectValidationError, PayloadOrAttributes}, AddOnsContext, ConfigureEvmEnv, EngineTypes,
    EngineValidator, FullNodeComponents, FullNodeTypes, NextBlockEnvAttributes, NodeTypes,
    NodeTypesWithDB, NodeTypesWithEngine, PayloadAttributes,
    PayloadAttributesBuilder, PayloadBuilderAttributes, PayloadTypes,
};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::{
    node::{
        EthereumConsensusBuilder, EthereumExecutorBuilder, EthereumNetworkBuilder,
        EthereumPoolBuilder,
    },
    EthEvmConfig,
};
use reth_payload_builder::{
    EthBuiltPayload, EthPayloadBuilderAttributes, PayloadBuilderError, PayloadBuilderHandle,
    PayloadBuilderService,
};
use reth_tracing::{RethTracer, Tracer};
use reth_trie_db::MerklePatriciaTrie;

pub mod builder;
pub mod cli;
pub mod engine_api;
pub mod exex;

sol!(RollupContract, "Gwyneth.json");

/// Gwyneth Payload Attributes
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GwynethPayloadAttributes {
    /// The payload attributes
    #[serde(flatten)]
    pub inner: EthPayloadAttributes,
    /// Transactions is a field for rollups: the transactions list is forced into the block
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transactions: Option<Vec<TransactionSigned>>,
    /// Transactions is a field for rollups: the transactions list is forced into the block
    pub chain_da: ChainDA,
    /// If set, this sets the exact gas limit the block produced with.
    #[serde(skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub gas_limit: Option<u64>,
}

/// Gwyneth error type used in payload attributes validation
#[derive(Debug, Error)]
pub enum GwynethError {
    #[error("Gwyneth field is not zero")]
    GwynethFieldIsNotZero,
}

impl PayloadAttributes for GwynethPayloadAttributes {
    fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        self.inner.withdrawals()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.inner.parent_beacon_block_root()
    }
}

/// New type around the payload builder attributes type
#[derive(Clone, Debug)]
pub struct GwynethPayloadBuilderAttributes {
    /// Inner ethereum payload builder attributes
    pub inner: EthPayloadBuilderAttributes,
    /// Decoded transactions and the original EIP-2718 encoded bytes as received in the payload
    /// attributes.
    pub transactions: Vec<WithEncoded<TransactionSigned>>,
    /// The gas limit for the generated payload
    pub gas_limit: Option<u64>,
    /// Cross-chain provider for L1
    pub sync_provider: Option<HashMap<ChainId, Arc<StateProviderBox>>>,
}

impl PartialEq for GwynethPayloadBuilderAttributes {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
            && self.transactions == other.transactions
            && self.gas_limit == other.gas_limit
        // && self.sync_provider == other.sync_provider
    }
}

impl Eq for GwynethPayloadBuilderAttributes {}

impl<ChainSpec> PayloadAttributesBuilder<GwynethPayloadAttributes>
    for LocalPayloadAttributesBuilder<ChainSpec>
where
    ChainSpec: Send + Sync + EthereumHardforks + 'static,
{
    fn build(&self, timestamp: u64) -> GwynethPayloadAttributes {
        let attributes = self.build(timestamp);
        GwynethPayloadAttributes { inner: attributes, transactions: None, gas_limit: None, chain_da: Default::default() }
    }
}

impl PayloadBuilderAttributes for GwynethPayloadBuilderAttributes {
    type RpcPayloadAttributes = GwynethPayloadAttributes;
    type Error = Infallible;

    fn try_new(
        parent: B256,
        attributes: GwynethPayloadAttributes,
        _version: u8,
    ) -> Result<Self, Infallible> {
        let transactions = attributes
            .transactions
            .unwrap_or_default()
            .into_iter()
            .map(|tx| {
                let mut buf = Vec::with_capacity(128 + tx.transaction.input().len());
                tx.eip2718_encode(tx.signature(), &mut buf);
                WithEncoded::new(buf.into(), tx)
            })
            .collect();

        Ok(Self {
            inner: EthPayloadBuilderAttributes::new(parent, attributes.inner),
            transactions,
            gas_limit: attributes.gas_limit,
            sync_provider: None,
        })
    }

    fn payload_id(&self) -> PayloadId {
        self.inner.id
    }

    fn parent(&self) -> B256 {
        self.inner.parent
    }

    fn timestamp(&self) -> u64 {
        self.inner.timestamp
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.inner.parent_beacon_block_root
    }

    fn suggested_fee_recipient(&self) -> Address {
        self.inner.suggested_fee_recipient
    }

    fn prev_randao(&self) -> B256 {
        self.inner.prev_randao
    }

    fn withdrawals(&self) -> &Withdrawals {
        &self.inner.withdrawals
    }
}

/// Gwyneth engine types - uses a Gwyneth payload attributes RPC type, but uses the default
/// payload builder attributes type.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[non_exhaustive]
pub struct GwynethEngineTypes;

impl PayloadTypes for GwynethEngineTypes {
    type BuiltPayload = EthBuiltPayload;
    type PayloadAttributes = GwynethPayloadAttributes;
    type PayloadBuilderAttributes = GwynethPayloadBuilderAttributes;
}

impl EngineTypes for GwynethEngineTypes {
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = ExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = ExecutionPayloadEnvelopeV4;
}

/// Gwyneth engine validator
#[derive(Debug, Clone)]
pub struct GwynethEngineValidator {
    chain_spec: Arc<ChainSpec>,
}

impl<T> EngineValidator<T> for GwynethEngineValidator
where
    T: EngineTypes<PayloadAttributes = GwynethPayloadAttributes>,
{
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, T::PayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        // validate_version_specific_fields(&self.chain_spec, version, payload_or_attrs)
        Ok(())
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &T::PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        // validate_version_specific_fields(&self.chain_spec, version, attributes.into())?;

        // // Gwyneth validation logic - ensure that the Gwyneth field is not zero
        // if attributes.custom == 0 {
        //     return Err(EngineObjectValidationError::invalid_params(
        //         GwynethError::GwynethFieldIsNotZero,
        //     ))
        // }

        Ok(())
    }
}

/// Gwyneth engine validator builder
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct GwynethEngineValidatorBuilder;

impl<N> EngineValidatorBuilder<N> for GwynethEngineValidatorBuilder
where
    N: FullNodeComponents<
        Types: NodeTypesWithEngine<Engine = GwynethEngineTypes, ChainSpec = ChainSpec>,
    >,
{
    type Validator = GwynethEngineValidator;

    async fn build(self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::Validator> {
        Ok(GwynethEngineValidator { chain_spec: ctx.config.chain.clone() })
    }
}

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
struct GwynethNode;

impl NodeTypes for GwynethNode {
    type Primitives = EthPrimitives;
    type ChainSpec = ChainSpec;
    type StateCommitment = MerklePatriciaTrie;
}

impl NodeTypesWithEngine for GwynethNode {
    type Engine = GwynethEngineTypes;
}

impl NodeTypesWithDB for GwynethNode {
    type DB = Arc<DatabaseEnv>;
}

pub type GwynethAddOns<N> = RpcAddOns<
    N,
    EthApi<
        <N as FullNodeTypes>::Provider,
        <N as FullNodeComponents>::Pool,
        NetworkHandle,
        <N as FullNodeComponents>::Evm,
    >,
    GwynethEngineValidatorBuilder,
>;

impl<Types, N> Node<N> for GwynethNode
where
    Types:
        NodeTypesWithDB + NodeTypesWithEngine<Engine = GwynethEngineTypes, ChainSpec = ChainSpec>,
    N: FullNodeTypes<Types = Types>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        GwynethPayloadServiceBuilder,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        EthereumConsensusBuilder,
    >;

    type AddOns = GwynethAddOns<
        NodeAdapter<N, <Self::ComponentsBuilder as NodeComponentsBuilder<N>>::Components>,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(EthereumPoolBuilder::default())
            .payload(GwynethPayloadServiceBuilder::default())
            .network(EthereumNetworkBuilder::default())
            .executor(EthereumExecutorBuilder::default())
            .consensus(EthereumConsensusBuilder::default())
    }

    fn add_ons(&self) -> Self::AddOns {
        GwynethAddOns::default()
    }
}

/// A Gwyneth payload service builder that supports the Gwyneth engine types
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct GwynethPayloadServiceBuilder;

impl<Node, Pool> PayloadServiceBuilder<Node, Pool> for GwynethPayloadServiceBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypesWithEngine<Engine = GwynethEngineTypes, ChainSpec = ChainSpec>,
    >,
    Pool: TransactionPool + Unpin + 'static,
{
    async fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypesWithEngine>::Engine>> {
        let payload_builder = GwynethPayloadBuilder::default();
        let conf = ctx.payload_builder_config();

        let payload_job_config = BasicPayloadJobGeneratorConfig::default()
            .interval(conf.interval())
            .deadline(conf.deadline())
            .max_payload_tasks(conf.max_payload_tasks())
            .extradata(conf.extradata_bytes());

        let payload_generator = BasicPayloadJobGenerator::with_builder(
            ctx.provider().clone(),
            pool,
            ctx.task_executor().clone(),
            payload_job_config,
            payload_builder,
        );
        let (payload_service, payload_builder) =
            PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

        ctx.task_executor().spawn_critical("payload builder service", Box::pin(payload_service));

        Ok(payload_builder)
    }
}

/// The type responsible for building Gwyneth payloads
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct GwynethPayloadBuilder;

impl<Pool, Client> PayloadBuilder<Pool, Client> for GwynethPayloadBuilder
where
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec>,
    Pool: TransactionPool,
{
    type Attributes = GwynethPayloadBuilderAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        let BuildArguments { client, pool, cached_reads, config, cancel, best_payload } = &args;
        let next_attributes = NextBlockEnvAttributes {
            timestamp: config.attributes.timestamp(),
            suggested_fee_recipient: config.attributes.suggested_fee_recipient(),
            prev_randao: config.attributes.prev_randao(),
        };
        let PayloadConfig { parent_header, extra_data, attributes } = config;

        let chain_spec = client.chain_spec();
        let evm_config = EthEvmConfig::new(chain_spec.clone());
        let (cfg_env, block_env) =
            evm_config.next_cfg_and_block_env(parent_header.header(), next_attributes).unwrap();

        default_gwyneth_payload(evm_config, args, cfg_env, block_env)
    }

    fn build_empty_payload(
        &self,
        client: &Client,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let PayloadConfig { parent_header, extra_data, attributes } = config;
        let chain_spec = client.chain_spec();
        <reth_ethereum_payload_builder::EthereumPayloadBuilder as PayloadBuilder<Pool, Client>>::build_empty_payload(&reth_ethereum_payload_builder::EthereumPayloadBuilder::new(EthEvmConfig::new(chain_spec.clone())),client,
                                                                                                                     PayloadConfig { parent_header, extra_data, attributes: attributes.inner})
    }
}

#[tokio::main]
async fn mainn() -> eyre::Result<()> {
    let _guard = RethTracer::new().init()?;

    let tasks = TaskManager::current();

    // create optimism genesis with canyon at block 2
    let spec = ChainSpec::builder()
        .chain(Chain::mainnet())
        .genesis(Genesis::default())
        .london_activated()
        .paris_activated()
        .shanghai_activated()
        .build();

    // create node config
    let node_config =
        NodeConfig::test().with_rpc(RpcServerArgs::default().with_http()).with_chain(spec);

    let handle = NodeBuilder::new(node_config)
        .with_gwyneth_launch_context(TaskManager::current().executor(), PathBuf::new())
        .launch_node(GwynethNode::default())
        .await
        .unwrap();
    let a: GwynethFullNode1 = handle.node.clone();

    println!("Node started");

    handle.node_exit_future.await
}
