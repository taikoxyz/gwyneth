use alloy_primitives::B256;
use alloy_rpc_types::engine::{ExecutionPayloadV3, ForkchoiceState, PayloadStatusEnum};
use jsonrpsee::{
    core::client::ClientT,
    http_client::{transport::HttpBackend, HttpClient},
};
use reth_ethereum_engine_primitives::ExecutionPayloadEnvelopeV3;
use reth_node_api::EngineTypes;
use reth_payload_builder::PayloadId;
use reth_provider::CanonStateNotificationStream;
use reth_rpc_api::EngineApiClient;
use reth_rpc_layer::AuthClientService;
use std::marker::PhantomData;
/// Helper for engine api operations
pub struct EngineApiContext<E> {
    pub canonical_stream: CanonStateNotificationStream,
    pub engine_api_client: HttpClient<AuthClientService<HttpBackend>>,
    pub _marker: PhantomData<E>,
}

impl<E: EngineTypes> EngineApiContext<E> {
    /// Retrieves a v3 payload from the engine api
    pub async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> eyre::Result<E::ExecutionPayloadEnvelopeV3> {
        Ok(EngineApiClient::<E>::get_payload_v3(&self.engine_api_client, payload_id).await?)
    }

    /// Retrieves a v3 payload from the engine api as serde value
    pub async fn get_payload_v3_value(
        &self,
        payload_id: PayloadId,
    ) -> eyre::Result<serde_json::Value> {
        Ok(self.engine_api_client.request("engine_getPayloadV3", (payload_id,)).await?)
    }

    /// Submits a payload to the engine api
    pub async fn submit_payload(
        &self,
        payload: E::BuiltPayload,
        parent_beacon_block_root: B256,
        expected_status: PayloadStatusEnum,
        versioned_hashes: Vec<B256>,
    ) -> eyre::Result<B256>
    where
        E::ExecutionPayloadEnvelopeV3: From<E::BuiltPayload> + PayloadEnvelopeExt,
    {
        // setup payload for submission
        let envelope_v3: <E as EngineTypes>::ExecutionPayloadEnvelopeV3 = payload.into();

        // submit payload to engine api
        let submission = EngineApiClient::<E>::new_payload_v3(
            &self.engine_api_client,
            envelope_v3.execution_payload(),
            versioned_hashes,
            parent_beacon_block_root,
        )
        .await?;

        assert_eq!(submission.status, expected_status);

        Ok(submission.latest_valid_hash.unwrap_or_default())
    }

    /// Sends forkchoice update to the engine api
    pub async fn update_forkchoice(&self, finalized_head: B256, new_head: B256) -> eyre::Result<()> {
        let fork_choice_state = ForkchoiceState {
            head_block_hash: new_head,
            safe_block_hash: finalized_head,
            finalized_block_hash: finalized_head,
        };
        self.fork_choice_updated_v3_wait(fork_choice_state).await
    }

    async fn fork_choice_updated_v3_wait(
        &self,
        fork_choice_state: ForkchoiceState,
    ) -> eyre::Result<()> {
        println!("fork choice: {:?}", fork_choice_state);
        let mut status =
            EngineApiClient::<E>::fork_choice_updated_v3(
                &self.engine_api_client,
                fork_choice_state,
                None,
            )
            .await?;

        while !status.is_valid() {
            if status.is_invalid() {
                println!("Invalid forkchoiceUpdatedV3: {status:?}");
            }
            status =
                EngineApiClient::<E>::fork_choice_updated_v3(
                    &self.engine_api_client,
                    fork_choice_state,
                    None,
                )
                .await?;
        }
        Ok(())
    }

    /// Sends forkchoice update to the engine api with a zero finalized hash
    pub async fn update_optimistic_forkchoice(&self, hash: B256) -> eyre::Result<()> {
        EngineApiClient::<E>::fork_choice_updated_v3(
            &self.engine_api_client,
            ForkchoiceState {
                head_block_hash: hash,
                safe_block_hash: B256::ZERO,
                finalized_block_hash: B256::ZERO,
            },
            None,
        )
        .await?;

        Ok(())
    }
}

/// The execution payload envelope type.
pub trait PayloadEnvelopeExt: Send + Sync + std::fmt::Debug {
    /// Returns the execution payload V3 from the payload
    fn execution_payload(&self) -> ExecutionPayloadV3;
}

impl PayloadEnvelopeExt for ExecutionPayloadEnvelopeV3 {
    fn execution_payload(&self) -> ExecutionPayloadV3 {
        self.execution_payload.clone()
    }
}
