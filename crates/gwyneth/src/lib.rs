//! Ethereum Node types config.
use std::{collections::HashMap, fmt::Debug, sync::Arc};

use builder::default_gwyneth_payload;
use reth_primitives::{ChainDA, StateDiff, TransactionSigned};
use reth_tasks::TaskManager;
use thiserror::Error;

use reth_basic_payload_builder::{
    BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig, BuildArguments, BuildOutcome,
    PayloadBuilder, PayloadConfig,
};
use reth_chainspec::{Chain, ChainSpec};
use reth_ethereum_engine_primitives::{
    EthBuiltPayload, EthPayloadAttributes, EthPayloadBuilderAttributes, ExecutionPayloadEnvelopeV2,
    ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4,
};
use reth_node_api::{
    PayloadBuilderError,
    payload::{EngineApiMessageVersion, EngineObjectValidationError, PayloadOrAttributes},
    validate_version_specific_fields, EngineTypes, PayloadAttributes, PayloadBuilderAttributes,
};
use reth_node_builder::{
    components::{ComponentsBuilder, PayloadServiceBuilder},
    node::{FullNodeTypes, NodeTypes},
    BuilderContext, Node, NodeBuilder, NodeConfig, PayloadBuilderConfig, PayloadTypes,
};
use reth_node_core::{
    args::RpcServerArgs,
    primitives::{
        transaction::WithEncoded, Block, TxType,
    },
};
use reth_node_ethereum::node::{
    EthereumConsensusBuilder, EthereumExecutorBuilder,
    EthereumPoolBuilder,
};
use reth_payload_builder::{
    PayloadBuilderHandle, PayloadBuilderService, PayloadId,
};
use reth_provider::{CanonStateSubscriptions, ChainSpecProvider, StateProviderBox, StateProviderFactory};
use alloy_rpc_types_engine::{ExecutionPayloadV1};
use reth_tracing::{RethTracer, Tracer};
use reth_transaction_pool::TransactionPool;
use serde::{Deserialize, Serialize};
use alloy_eips::eip4895::{Withdrawal, Withdrawals};
use alloy_primitives::{Address, B256, ChainId};
use reth_node_api::NodePrimitives;
use alloy_consensus::Receipt;
use reth_trie_db::MerklePatriciaTrie;
use reth_node_api::NodeTypesWithEngine;
use reth_node_builder::NodeAdapter;
use reth_node_builder::NodeComponentsBuilder;
use reth_provider::StateProvider;
use reth_node_api::NodeTypesWithDB;
use reth_node_builder::rpc::RpcAddOns;
use reth_node_api::FullNodeComponents;
use reth_network::NetworkHandle;
use reth_rpc::EthApi;
//use reth_node_ethereum::node::EthereumEngineValidatorBuilder;
use reth_node_builder::rpc::EngineValidatorBuilder;
use reth_primitives::EthPrimitives;
use reth_evm::ConfigureEvm;
use alloy_consensus::Header;
use reth_node_api::EngineValidator;

use reth_node_ethereum::{
    node::{EthereumAddOns, EthereumNetworkBuilder, EthereumPayloadBuilder},
    EthEngineTypes, EthEvmConfig,
};

pub mod builder;
pub mod engine_api;
pub mod exex;

/// Gwyneth error type used in payload attributes validation
#[derive(Debug, Error)]
pub enum GwynetError {
    #[error("Gwyneth field is not zero")]
    RlpError(alloy_rlp::Error),
}

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

impl PayloadAttributes for GwynethPayloadAttributes {
    fn timestamp(&self) -> u64 {
        self.inner.timestamp
    }

    fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        self.inner.withdrawals.as_ref()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.inner.parent_beacon_block_root
    }
}

/// Gwyneth Payload Builder Attributes
#[derive(Clone, Debug)]
pub struct GwynethPayloadBuilderAttributes {
    /// Inner ethereum payload builder attributes
    pub inner: EthPayloadBuilderAttributes,
    /// Decoded transactions and the original EIP-2718 encoded bytes as received in the payload
    /// attributes.
    pub transactions: Vec<TransactionSigned>,
    /// chain DA
    pub chain_da: ChainDA,
    /// The gas limit for the generated payload
    pub gas_limit: Option<u64>,

    //pub providers: HashMap<ChainId, SyncProvider>,
}

impl PayloadBuilderAttributes for GwynethPayloadBuilderAttributes
{
    type RpcPayloadAttributes = GwynethPayloadAttributes;
    type Error = alloy_rlp::Error;

    fn try_new(
        parent: B256,
        attributes: GwynethPayloadAttributes,
        version: u8,
    ) -> Result<Self, alloy_rlp::Error> {
        // let transactions = attributes
        //     .transactions
        //     .unwrap_or_default()
        //     .into_iter()
        //     .map(|tx| WithEncoded::new(tx.envelope_encoded(), tx))
        //     .collect();

        Ok(Self {
            inner: EthPayloadBuilderAttributes::new(parent, attributes.inner),
            transactions: attributes.transactions.unwrap_or_default(),
            chain_da: attributes.chain_da,
            gas_limit: attributes.gas_limit,
            //providers: HashMap::default(),
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
    //type SyncProvider = Arc<StateProviderBox>;
}

impl EngineTypes for GwynethEngineTypes {
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = ExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = ExecutionPayloadEnvelopeV4;
}

/// Gwyneth primitive types.
// #[derive(Debug, Default, Clone, PartialEq, Eq)]
// pub struct GwynethPrimitives;

// impl NodePrimitives for GwynethPrimitives {
//     type Block = Block;
//     type SignedTx = TransactionSigned;
//     type TxType = TxType;
//     type Receipt = Receipt;
// }

#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct GwynethNode;

impl GwynethNode {
    /// Returns a [`ComponentsBuilder`] configured for a regular Ethereum node.
    pub fn components<Node>() -> ComponentsBuilder<
        Node,
        EthereumPoolBuilder,
        EthereumPayloadBuilder,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        EthereumConsensusBuilder,
    >
    where
        Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec>>,
        <Node::Types as NodeTypesWithEngine>::Engine: PayloadTypes<
            BuiltPayload = EthBuiltPayload,
            PayloadAttributes = EthPayloadAttributes,
            PayloadBuilderAttributes = EthPayloadBuilderAttributes,
        >,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(EthereumPoolBuilder::default())
            .payload(EthereumPayloadBuilder::default())
            .network(EthereumNetworkBuilder::default())
            .executor(EthereumExecutorBuilder::default())
            .consensus(EthereumConsensusBuilder::default())
    }
}

/// Configure the node types
impl NodeTypes for GwynethNode {
    type Primitives = EthPrimitives;
    type ChainSpec = ChainSpec;
    type StateCommitment = MerklePatriciaTrie;
}

impl NodeTypesWithEngine for GwynethNode {
    type Engine = EthEngineTypes; // TODO
}

// Add-ons w.r.t. l1 ethereum.
// pub type GwynethAddOns<N> = RpcAddOns<
//     N,
//     EthApi<
//         <N as FullNodeTypes>::Provider,
//         <N as FullNodeComponents>::Pool,
//         NetworkHandle,
//         <N as FullNodeComponents>::Evm,
//     >,
//     GwynethEngineValidatorBuilder,
// >;

/// Implement the Node trait for the Gwyneth node
///
/// This provides a preset configuration for the node
impl<N> Node<N> for GwynethNode
where
    N: FullNodeTypes<Types: NodeTypesWithEngine<Engine = EthEngineTypes, ChainSpec = ChainSpec>>,
    //Types: NodeTypesWithDB + NodeTypesWithEngine<Engine = GwynethEngineTypes, ChainSpec = ChainSpec>,
    //N: FullNodeTypes<Types = Types>,
    // EthereumEngineValidatorBuilder:
    //     EngineValidatorBuilder<
    //         NodeAdapter<
    //             N,
    //             <Self::ComponentsBuilder as NodeComponentsBuilder<N>>::Components
    //         >
    //     >,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        EthereumPayloadBuilder,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        EthereumConsensusBuilder,
    >;

    //type AddOns = GwynethAddOns<GwynethNode>;

    // type AddOns = GwynethAddOns<
    //     NodeAdapter<N, <Self::ComponentsBuilder as NodeComponentsBuilder<N>>::Components>,
    // >;

    type AddOns = EthereumAddOns<
        NodeAdapter<N, <Self::ComponentsBuilder as NodeComponentsBuilder<N>>::Components>,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        Self::components()
    }

    fn add_ons(&self) -> Self::AddOns {
        //GwynethAddOns::default()
        EthereumAddOns::default()
    }
}


/// A basic ethereum payload service.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct GwynethPayloadBuilder;

impl GwynethPayloadBuilder {
    /// A helper method initializing [`PayloadBuilderService`] with the given EVM config.
    pub fn spawn<Types, Node, Evm, Pool>(
        self,
        evm_config: Evm,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<Types::Engine>>
    where
        Types: NodeTypesWithEngine<ChainSpec = ChainSpec>,
        Node: FullNodeTypes<Types = Types>,
        Evm: ConfigureEvm<Header = Header>,
        Pool: TransactionPool + Unpin + 'static,
        Types::Engine: PayloadTypes<
            BuiltPayload = EthBuiltPayload,
            PayloadAttributes = GwynethPayloadAttributes,
            PayloadBuilderAttributes = GwynethPayloadBuilderAttributes,
        >,
    {
        let payload_builder = crate::builder::GwynethPayloadBuilder::new(evm_config);
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

impl<Types, Node, Pool> PayloadServiceBuilder<Node, Pool> for GwynethPayloadBuilder
where
    Types: NodeTypesWithEngine<ChainSpec = ChainSpec>,
    Node: FullNodeTypes<Types = Types>,
    Pool: TransactionPool + Unpin + 'static,
    Types::Engine: PayloadTypes<
        BuiltPayload = EthBuiltPayload,
        PayloadAttributes = GwynethPayloadAttributes,
        PayloadBuilderAttributes = GwynethPayloadBuilderAttributes,
    >,
{
    async fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<Types::Engine>> {
        self.spawn(EthEvmConfig::new(ctx.chain_spec()), ctx, pool)
    }
}

// Builder for [`GwynethEngineValidator`].
// #[derive(Debug, Default, Clone)]
// #[non_exhaustive]
// pub struct GwynethEngineValidatorBuilder;

// impl<Node, Types> EngineValidatorBuilder<Node> for GwynethEngineValidatorBuilder
// where
//     Types: NodeTypesWithEngine<ChainSpec = ChainSpec>,
//     Node: FullNodeComponents<Types = Types>,
//     GwynethEngineValidator: EngineValidator<Types::Engine>,
// {
//     type Validator = GwynethEngineValidator;

//     async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
//         Ok(GwynethEngineValidator::new(ctx.config.chain.clone()))
//     }
// }

/// Validator for the ethereum engine API.
#[derive(Debug, Clone)]
pub struct GwynethEngineValidator {
    chain_spec: Arc<ChainSpec>,
}

impl GwynethEngineValidator {
    /// Instantiates a new validator.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl<Types> EngineValidator<Types> for GwynethEngineValidator
where
    Types: EngineTypes<PayloadAttributes = GwynethPayloadAttributes>,
{
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, GwynethPayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(&self.chain_spec, version, payload_or_attrs)
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &GwynethPayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(&self.chain_spec, version, attributes.into())
    }
}


// #[derive(Debug, Default, Clone)]
// #[non_exhaustive]
// pub struct GwynethPayloadBuilder;

// impl<Pool, Client> PayloadBuilder<Pool, Client> for GwynethPayloadBuilder
// where
//     Client: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec>,
//     Pool: TransactionPool,
// {
//     type Attributes = GwynethPayloadBuilderAttributes<Arc<StateProviderBox>>;
//     type BuiltPayload = EthBuiltPayload;

//     // fn try_build(
//     //     &self,
//     //     args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
//     // ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
//     //     let BuildArguments { client, pool, cached_reads, config, cancel, best_payload } = args;
//     //     let PayloadConfig { parent_header, extra_data, attributes } = config;

//     //     let chain_spec = client.chain_spec();

//     //     // This reuses the default EthereumPayloadBuilder to build the payload
//     //     // but any custom logic can be implemented here
//     //     reth_ethereum_payload_builder::EthereumPayloadBuilder::new(EthEvmConfig::new(
//     //         chain_spec.clone(),
//     //     ))
//     //     .try_build(BuildArguments {
//     //         client,
//     //         pool,
//     //         cached_reads,
//     //         config: PayloadConfig { parent_header, extra_data, attributes: attributes.0 },
//     //         cancel,
//     //         best_payload,
//     //     })
//     // }

//     fn try_build(
//         &self,
//         args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
//     ) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError> {
//         let (cfg_env, block_env) = self
//             .cfg_and_block_env(&args.config, &args.config.parent_header)
//             .map_err(PayloadBuilderError::other)?;

//         let pool = args.pool.clone();
//         default_gwyneth_payload(self.evm_config.clone(), args, cfg_env, block_env, |attributes| {
//             pool.best_transactions_with_attributes(attributes)
//         })
//     }

//     fn build_empty_payload(
//         &self,
//         client: &Client,
//         config: PayloadConfig<Self::Attributes>,
//     ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
//         let PayloadConfig { parent_header, extra_data, attributes } = config;
//         let chain_spec = client.chain_spec();
//         <reth_ethereum_payload_builder::EthereumPayloadBuilder as PayloadBuilder<Pool, Client>>::build_empty_payload(&reth_ethereum_payload_builder::EthereumPayloadBuilder::new(EthEvmConfig::new(chain_spec.clone())),client,
//                                                                                                                      PayloadConfig { parent_header, extra_data, attributes: attributes.0})
//     }
// }



// The type responsible for building Gwyneth payloads
// #[derive(Debug, Default, Clone)]
// #[non_exhaustive]
// pub struct GwynethPayloadBuilder;

// impl<Pool, Client> PayloadBuilder<Pool, Client> for GwynethPayloadBuilder
// where
//     Client: StateProviderFactory,
//     Pool: TransactionPool,
// {
//     type Attributes = GwynethPayloadBuilderAttributes<Arc<StateProviderBox>>;
//     type BuiltPayload = EthBuiltPayload;

//     fn try_build(
//         &self,
//         args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
//     ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
//         default_gwyneth_payload_builder(EthEvmConfig::default(), args)
//     }

//     fn build_empty_payload(
//         &self,
//         client: &Client,
//         config: PayloadConfig<Self::Attributes>,
//     ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
//         let PayloadConfig {
//             initialized_block_env,
//             initialized_cfg,
//             parent_block,
//             extra_data,
//             attributes,
//             chain_spec,
//         } = config;
//         let eth_payload_config = PayloadConfig {
//             initialized_block_env,
//             initialized_cfg,
//             parent_block,
//             extra_data,
//             attributes: attributes.inner,
//             chain_spec,
//         };
//         <reth_ethereum_payload_builder::EthereumPayloadBuilder as PayloadBuilder<Pool, Client>>::build_empty_payload(&reth_ethereum_payload_builder::EthereumPayloadBuilder::default(),client, eth_payload_config)
//     }

//     fn on_missing_payload(
//         &self,
//         _args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
//     ) -> reth_basic_payload_builder::MissingPayloadBehaviour<Self::BuiltPayload> {
//         reth_basic_payload_builder::MissingPayloadBehaviour::RaceEmptyPayload
//     }
// }

// A basic ethereum payload service.
// #[derive(Debug, Default, Clone)]
// #[non_exhaustive]
// pub struct GwynethPayloadBuilder;

// impl GwynethPayloadBuilder {
//     /// A helper method initializing [`GwynethPayloadBuilder`] with the given EVM config.
//     pub fn spawn<Types, Node, Evm, Pool>(
//         self,
//         evm_config: Evm,
//         ctx: &BuilderContext<Node>,
//         pool: Pool,
//     ) -> eyre::Result<PayloadBuilderHandle<Types::Engine>>
//     where
//         Types: NodeTypesWithEngine<ChainSpec = ChainSpec>,
//         Node: FullNodeTypes<Types = Types>,
//         Evm: ConfigureEvm<Header = Header>,
//         Pool: TransactionPool + Unpin + 'static,
//         Types::Engine: PayloadTypes<
//             BuiltPayload = EthBuiltPayload,
//             PayloadAttributes = GwynethPayloadAttributes,
//             PayloadBuilderAttributes = GwynethPayloadBuilderAttributes<Arc<StateProviderBox>>,
//         >,
//     {
//         let payload_builder =
//             reth_ethereum_payload_builder::EthereumPayloadBuilder::new(evm_config);
//         let conf = ctx.payload_builder_config();

//         let payload_job_config = BasicPayloadJobGeneratorConfig::default()
//             .interval(conf.interval())
//             .deadline(conf.deadline())
//             .max_payload_tasks(conf.max_payload_tasks())
//             .extradata(conf.extradata_bytes());

//         let payload_generator = BasicPayloadJobGenerator::with_builder(
//             ctx.provider().clone(),
//             pool,
//             ctx.task_executor().clone(),
//             payload_job_config,
//             payload_builder,
//         );
//         let (payload_service, payload_builder) =
//             PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

//         ctx.task_executor().spawn_critical("payload builder service", Box::pin(payload_service));

//         Ok(payload_builder)
//     }
// }

// impl<Types, Node, Pool> PayloadServiceBuilder<Node, Pool> for GwynethPayloadBuilder
// where
//     Types: NodeTypesWithEngine<ChainSpec = ChainSpec>,
//     Node: FullNodeTypes<Types = Types>,
//     Pool: TransactionPool + Unpin + 'static,
//     Types::Engine: PayloadTypes<
//         BuiltPayload = EthBuiltPayload,
//         PayloadAttributes = GwynethPayloadAttributes,
//         PayloadBuilderAttributes = GwynethPayloadBuilderAttributes<Arc<StateProviderBox>>,
//     >,
// {
//     async fn spawn_payload_service(
//         self,
//         ctx: &BuilderContext<Node>,
//         pool: Pool,
//     ) -> eyre::Result<PayloadBuilderHandle<Types::Engine>> {
//         self.spawn(EthEvmConfig::new(ctx.chain_spec()), ctx, pool)
//     }
// }

// impl<Node, Pool> PayloadServiceBuilder<Node, Pool> for GwynethPayloadBuilder
// where
//     Node: FullNodeTypes<Types: NodeTypesWithEngine<Engine = GwynethEngineTypes, ChainSpec = ChainSpec>>,
//     Pool: TransactionPool + Unpin + 'static,
// {
//     async fn spawn_payload_service(
//         self,
//         ctx: &BuilderContext<Node>,
//         pool: Pool,
//     ) -> eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypesWithEngine>::Engine>> {
//         tracing::info!("Spawning a custom payload builder");
//         let conf = ctx.payload_builder_config();

//         let payload_job_config = BasicPayloadJobGeneratorConfig::default()
//             .interval(conf.interval())
//             .deadline(conf.deadline())
//             .max_payload_tasks(conf.max_payload_tasks())
//             .extradata(conf.extradata_bytes());

//         unimplemented!("spawn_payload_service");

//         // let payload_generator = EmptyBlockPayloadJobGenerator::with_builder(
//         //     ctx.provider().clone(),
//         //     pool,
//         //     ctx.task_executor().clone(),
//         //     payload_job_config,
//         //     reth_ethereum_payload_builder::EthereumPayloadBuilder::new(EthEvmConfig::new(
//         //         ctx.chain_spec(),
//         //     )),
//         // );

//         // let (payload_service, payload_builder) =
//         //     PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

//         // ctx.task_executor()
//         //     .spawn_critical("custom payload builder service", Box::pin(payload_service));

//         // Ok(payload_builder)
//     }
// }


// impl<Types, Node, Pool> PayloadServiceBuilder<Node, Pool> for GwynethPayloadBuilder
// where
//     Types: NodeTypesWithEngine<ChainSpec = ChainSpec>,
//     Node: FullNodeTypes<Types = Types>,
//     Pool: TransactionPool + Unpin + 'static,
//     Types::Engine: PayloadTypes<
//         BuiltPayload = EthBuiltPayload,
//         PayloadAttributes = EthPayloadAttributes,
//         PayloadBuilderAttributes = EthPayloadBuilderAttributes,
//     >,
// {
//     async fn spawn_payload_service(
//         self,
//         ctx: &BuilderContext<Node>,
//         pool: Pool,
//     ) -> eyre::Result<PayloadBuilderHandle<Types::Engine>> {
//         self.spawn(EthEvmConfig::new(ctx.chain_spec()), ctx, pool)
//     }
// }


// /// A basic ethereum payload service.
// #[derive(Debug, Default, Clone)]
// #[non_exhaustive]
// pub struct EthereumPayloadBuilder;

// impl EthereumPayloadBuilder {
//     /// A helper method initializing [`PayloadBuilderService`] with the given EVM config.
//     pub fn spawn<Types, Node, Evm, Pool>(
//         self,
//         evm_config: Evm,
//         ctx: &BuilderContext<Node>,
//         pool: Pool,
//     ) -> eyre::Result<PayloadBuilderHandle<Types::Engine>>
//     where
//         Types: NodeTypesWithEngine<ChainSpec = ChainSpec>,
//         Node: FullNodeTypes<Types = Types>,
//         Evm: ConfigureEvm<Header = Header>,
//         Pool: TransactionPool + Unpin + 'static,
//         Types::Engine: PayloadTypes<
//             BuiltPayload = EthBuiltPayload,
//             PayloadAttributes = EthPayloadAttributes,
//             PayloadBuilderAttributes = EthPayloadBuilderAttributes,
//         >,
//     {
//         let payload_builder =
//             reth_ethereum_payload_builder::EthereumPayloadBuilder::new(evm_config);
//         let conf = ctx.payload_builder_config();

//         let payload_job_config = BasicPayloadJobGeneratorConfig::default()
//             .interval(conf.interval())
//             .deadline(conf.deadline())
//             .max_payload_tasks(conf.max_payload_tasks())
//             .extradata(conf.extradata_bytes());

//         let payload_generator = BasicPayloadJobGenerator::with_builder(
//             ctx.provider().clone(),
//             pool,
//             ctx.task_executor().clone(),
//             payload_job_config,
//             payload_builder,
//         );
//         let (payload_service, payload_builder) =
//             PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

//         ctx.task_executor().spawn_critical("payload builder service", Box::pin(payload_service));

//         Ok(payload_builder)
//     }
// }

// impl<Types, Node, Pool> PayloadServiceBuilder<Node, Pool> for EthereumPayloadBuilder
// where
//     Types: NodeTypesWithEngine<ChainSpec = ChainSpec>,
//     Node: FullNodeTypes<Types = Types>,
//     Pool: TransactionPool + Unpin + 'static,
//     Types::Engine: PayloadTypes<
//         BuiltPayload = EthBuiltPayload,
//         PayloadAttributes = EthPayloadAttributes,
//         PayloadBuilderAttributes = EthPayloadBuilderAttributes,
//     >,
// {
//     async fn spawn_payload_service(
//         self,
//         ctx: &BuilderContext<Node>,
//         pool: Pool,
//     ) -> eyre::Result<PayloadBuilderHandle<Types::Engine>> {
//         self.spawn(EthEvmConfig::new(ctx.chain_spec()), ctx, pool)
//     }
// }









// impl<Node, Pool> PayloadServiceBuilder<Node, Pool> for GwynethPayloadBuilder
// where
//     Node: FullNodeTypes<Engine = GwynethEngineTypes>,
//     Pool: TransactionPool + Unpin + 'static,
// {
//     async fn spawn_payload_service(
//         self,
//         ctx: &BuilderContext<Node>,
//         pool: Pool,
//     ) -> eyre::Result<PayloadBuilderHandle<Node::Engine>> {
//         let payload_builder = Self::default();
//         let conf = ctx.payload_builder_config();

//         let payload_job_config = BasicPayloadJobGeneratorConfig::default()
//             .interval(conf.interval())
//             .deadline(conf.deadline())
//             .max_payload_tasks(conf.max_payload_tasks())
//             .extradata(conf.extradata_bytes());

//         let payload_generator = BasicPayloadJobGenerator::with_builder(
//             ctx.provider().clone(),
//             pool,
//             ctx.task_executor().clone(),
//             payload_job_config,
//             ctx.chain_spec(),
//             payload_builder,
//         );
//         let (payload_service, payload_builder) =
//             PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

//         ctx.task_executor().spawn_critical("payload builder service", Box::pin(payload_service));

//         Ok(payload_builder)
//     }
// }

// #[tokio::main]
// async fn main() -> eyre::Result<()> {
//     let _guard = RethTracer::new().init()?;

//     let tasks = TaskManager::current();

//     // create gwyneth genesis with canyon at block 2
//     let spec = ChainSpec::builder()
//         .chain(Chain::mainnet())
//         .genesis(Genesis::default())
//         .london_activated()
//         .paris_activated()
//         .shanghai_activated()
//         .build();

//     // create node config
//     let node_config =
//         NodeConfig::test().with_rpc(RpcServerArgs::default().with_http()).with_chain(spec);

//     let handle = NodeBuilder::new(node_config)
//         .testing_node(tasks.executor())
//         .launch_node(GwynethNode::default())
//         .await
//         .unwrap();

//     handle.node_exit_future.await
// }



// A basic optimism payload service builder
// #[derive(Debug, Default, Clone)]
// pub struct OpPayloadBuilder<Txs = ()> {
//     /// By default the pending block equals the latest block
//     /// to save resources and not leak txs from the tx-pool,
//     /// this flag enables computing of the pending block
//     /// from the tx-pool instead.
//     ///
//     /// If `compute_pending_block` is not enabled, the payload builder
//     /// will use the payload attributes from the latest block. Note
//     /// that this flag is not yet functional.
//     pub compute_pending_block: bool,
//     /// The type responsible for yielding the best transactions for the payload if mempool
//     /// transactions are allowed.
//     pub best_transactions: Txs,
// }

// impl OpPayloadBuilder {
//     /// Create a new instance with the given `compute_pending_block` flag.
//     pub const fn new(compute_pending_block: bool) -> Self {
//         Self { compute_pending_block, best_transactions: () }
//     }
// }

// impl<Txs> OpPayloadBuilder<Txs>
// where
//     Txs: OpPayloadTransactions,
// {
//     /// Configures the type responsible for yielding the transactions that should be included in the
//     /// payload.
//     pub fn with_transactions<T: OpPayloadTransactions>(
//         self,
//         best_transactions: T,
//     ) -> OpPayloadBuilder<T> {
//         let Self { compute_pending_block, .. } = self;
//         OpPayloadBuilder { compute_pending_block, best_transactions }
//     }

//     /// A helper method to initialize [`PayloadBuilderService`] with the given EVM config.
//     pub fn spawn<Node, Evm, Pool>(
//         self,
//         evm_config: Evm,
//         ctx: &BuilderContext<Node>,
//         pool: Pool,
//     ) -> eyre::Result<PayloadBuilderHandle<OpEngineTypes>>
//     where
//         Node: FullNodeTypes<
//             Types: NodeTypesWithEngine<Engine = OpEngineTypes, ChainSpec = OpChainSpec>,
//         >,
//         Pool: TransactionPool + Unpin + 'static,
//         Evm: ConfigureEvm<Header = Header>,
//     {
//         let payload_builder = reth_optimism_payload_builder::OpPayloadBuilder::new(evm_config)
//             .with_transactions(self.best_transactions)
//             .set_compute_pending_block(self.compute_pending_block);
//         let conf = ctx.payload_builder_config();

//         let payload_job_config = BasicPayloadJobGeneratorConfig::default()
//             .interval(conf.interval())
//             .deadline(conf.deadline())
//             .max_payload_tasks(conf.max_payload_tasks())
//             // no extradata for OP
//             .extradata(Default::default());

//         let payload_generator = BasicPayloadJobGenerator::with_builder(
//             ctx.provider().clone(),
//             pool,
//             ctx.task_executor().clone(),
//             payload_job_config,
//             payload_builder,
//         );
//         let (payload_service, payload_builder) =
//             PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

//         ctx.task_executor().spawn_critical("payload builder service", Box::pin(payload_service));

//         Ok(payload_builder)
//     }
// }

// impl<Node, Pool, Txs> PayloadServiceBuilder<Node, Pool> for OpPayloadBuilder<Txs>
// where
//     Node:
//         FullNodeTypes<Types: NodeTypesWithEngine<Engine = OpEngineTypes, ChainSpec = OpChainSpec>>,
//     Pool: TransactionPool + Unpin + 'static,
//     Txs: OpPayloadTransactions,
// {
//     async fn spawn_payload_service(
//         self,
//         ctx: &BuilderContext<Node>,
//         pool: Pool,
//     ) -> eyre::Result<PayloadBuilderHandle<OpEngineTypes>> {
//         self.spawn(OpEvmConfig::new(ctx.chain_spec()), ctx, pool)
//     }
// }



// Builder for [`EthereumEngineValidator`].
// #[derive(Debug, Default, Clone)]
// #[non_exhaustive]
// pub struct EthereumEngineValidatorBuilder;

// impl<Node, Types> EngineValidatorBuilder<Node> for EthereumEngineValidatorBuilder
// where
//     Types: NodeTypesWithEngine<ChainSpec = ChainSpec>,
//     Node: FullNodeComponents<Types = Types>,
//     EthereumEngineValidator: EngineValidator<Types::Engine>,
// {
//     type Validator = EthereumEngineValidator;

//     async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
//         Ok(EthereumEngineValidator::new(ctx.config.chain.clone()))
//     }
// }
