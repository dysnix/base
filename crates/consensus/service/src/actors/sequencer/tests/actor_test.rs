use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use alloy_primitives::B256;
use alloy_rpc_types_engine::{ExecutionPayloadV1, PayloadId};
use async_trait::async_trait;
use base_common_rpc_types_engine::{
    BaseExecutionPayload, BaseExecutionPayloadEnvelope, BasePayloadAttributes,
};
use base_consensus_derive::{BuilderError, PipelineErrorKind, test_utils::TestAttributesBuilder};
use base_consensus_engine::SealTaskError;
use base_protocol::{AttributesWithParent, BlockInfo, L2BlockInfo};
use jsonrpsee::core::ClientError;
use rstest::rstest;
use tokio::{
    sync::{Semaphore, mpsc, oneshot},
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;

use crate::{
    Conductor, ConductorError, EngineClientResult, NodeActor, OriginSelector, SealState,
    SealStepError, SequencerActor, SequencerActorError, SequencerAdminQuery, SequencerEngineClient,
    SequencerRuntime, SequencerRuntimeFuture, SequencerTicker, UnsafePayloadGossipClient,
    UnsafePayloadGossipClientError, UnsealedPayloadHandle,
    actors::{
        MockConductor, MockOriginSelector, MockSequencerEngineClient,
        MockUnsafePayloadGossipClient,
        engine::EngineClientError,
        sequencer::{
            PayloadBuilder, PayloadSealer, RecoveryModeGuard, tests::test_util::test_actor,
        },
    },
};

fn dummy_envelope() -> BaseExecutionPayloadEnvelope {
    BaseExecutionPayloadEnvelope {
        parent_beacon_block_root: None,
        execution_payload: BaseExecutionPayload::V1(ExecutionPayloadV1 {
            parent_hash: B256::ZERO,
            fee_recipient: alloy_primitives::Address::ZERO,
            state_root: B256::ZERO,
            receipts_root: B256::ZERO,
            logs_bloom: alloy_primitives::Bloom::ZERO,
            prev_randao: B256::ZERO,
            block_number: 1,
            gas_limit: 0,
            gas_used: 0,
            timestamp: 0,
            extra_data: alloy_primitives::Bytes::new(),
            base_fee_per_gas: alloy_primitives::U256::ZERO,
            block_hash: B256::ZERO,
            transactions: vec![],
        }),
    }
}

fn conductor_rpc_error() -> ConductorError {
    ConductorError::Rpc(ClientError::Custom("test conductor error".to_string()))
}

fn dummy_attributes_with_parent() -> AttributesWithParent {
    AttributesWithParent::new(BasePayloadAttributes::default(), L2BlockInfo::default(), None, false)
}

fn handle_with_parent_number(number: u64) -> UnsealedPayloadHandle {
    handle_with_parent(number, B256::ZERO)
}

fn handle_with_parent(number: u64, hash: B256) -> UnsealedPayloadHandle {
    let parent = L2BlockInfo {
        block_info: BlockInfo { number, hash, ..Default::default() },
        ..Default::default()
    };
    UnsealedPayloadHandle {
        payload_id: Default::default(),
        attributes_with_parent: AttributesWithParent::new(
            BasePayloadAttributes::default(),
            parent,
            None,
            false,
        ),
    }
}

fn head_at(number: u64) -> L2BlockInfo {
    head_at_with_hash(number, B256::ZERO)
}

fn head_at_with_hash(number: u64, hash: B256) -> L2BlockInfo {
    L2BlockInfo {
        block_info: BlockInfo { number, hash, ..Default::default() },
        ..Default::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequencerLoopEvent {
    Reset,
    StartBuild,
    GetSealedPayload,
    CommitStarted,
    CommitFinished,
    GossipScheduled,
    InsertUnsafePayload,
}

#[derive(Debug, Clone)]
struct ManualSequencerRuntime {
    sleeps: Arc<Semaphore>,
    ticks: Arc<Semaphore>,
}

impl ManualSequencerRuntime {
    fn new() -> Self {
        Self { sleeps: Arc::new(Semaphore::new(0)), ticks: Arc::new(Semaphore::new(0)) }
    }

    fn tick(&self) {
        self.ticks.add_permits(1);
    }
}

impl SequencerRuntime for ManualSequencerRuntime {
    fn ticker(&self, _period: Duration) -> Box<dyn SequencerTicker> {
        Box::new(ManualSequencerTicker { ticks: Arc::clone(&self.ticks) })
    }

    fn sleep(&self, _duration: Duration) -> SequencerRuntimeFuture<'static, ()> {
        let sleeps = Arc::clone(&self.sleeps);
        Box::pin(async move {
            let permit = sleeps.acquire_owned().await.expect("manual sleep semaphore is open");
            drop(permit);
        })
    }
}

#[derive(Debug)]
struct ManualSequencerTicker {
    ticks: Arc<Semaphore>,
}

impl SequencerTicker for ManualSequencerTicker {
    fn reset_at(&mut self, _target: SystemTime) {}

    fn reset_immediately(&mut self) {}

    fn tick(&mut self) -> SequencerRuntimeFuture<'_, Instant> {
        let ticks = Arc::clone(&self.ticks);
        Box::pin(async move {
            let permit = ticks.acquire_owned().await.expect("manual tick semaphore is open");
            drop(permit);
            Instant::now()
        })
    }
}

#[derive(Debug)]
struct SequencerLoopEngine {
    events: mpsc::UnboundedSender<SequencerLoopEvent>,
    head: L2BlockInfo,
    reset_results: Mutex<VecDeque<EngineClientResult<()>>>,
}

impl SequencerLoopEngine {
    fn new(
        events: mpsc::UnboundedSender<SequencerLoopEvent>,
        head: L2BlockInfo,
        reset_results: Vec<EngineClientResult<()>>,
    ) -> Self {
        Self { events, head, reset_results: Mutex::new(reset_results.into()) }
    }

    fn send_event(&self, event: SequencerLoopEvent) {
        self.events.send(event).expect("test event receiver is open");
    }
}

#[async_trait]
impl SequencerEngineClient for SequencerLoopEngine {
    async fn reset_engine_forkchoice(&self) -> EngineClientResult<()> {
        self.send_event(SequencerLoopEvent::Reset);
        self.reset_results
            .lock()
            .expect("reset results lock is not poisoned")
            .pop_front()
            .unwrap_or(Ok(()))
    }

    async fn start_build_block(
        &self,
        _attributes: AttributesWithParent,
    ) -> EngineClientResult<PayloadId> {
        self.send_event(SequencerLoopEvent::StartBuild);
        Ok(PayloadId::default())
    }

    async fn get_sealed_payload(
        &self,
        _payload_id: PayloadId,
        _attributes: AttributesWithParent,
    ) -> EngineClientResult<BaseExecutionPayloadEnvelope> {
        self.send_event(SequencerLoopEvent::GetSealedPayload);
        Ok(dummy_envelope())
    }

    async fn insert_unsafe_payload(
        &self,
        _payload: BaseExecutionPayloadEnvelope,
    ) -> EngineClientResult<()> {
        self.send_event(SequencerLoopEvent::InsertUnsafePayload);
        Ok(())
    }

    async fn get_unsafe_head(&self) -> EngineClientResult<L2BlockInfo> {
        Ok(self.head)
    }
}

#[derive(Debug)]
struct StaticOriginSelector;

#[async_trait]
impl OriginSelector for StaticOriginSelector {
    async fn next_l1_origin(
        &mut self,
        _unsafe_head: L2BlockInfo,
        _is_recovery_mode: bool,
    ) -> Result<BlockInfo, crate::L1OriginSelectorError> {
        Ok(BlockInfo::default())
    }
}

#[derive(Debug, Clone)]
struct BlockingConductor {
    commits: Arc<Semaphore>,
    events: mpsc::UnboundedSender<SequencerLoopEvent>,
}

impl BlockingConductor {
    fn new(events: mpsc::UnboundedSender<SequencerLoopEvent>) -> Self {
        Self { commits: Arc::new(Semaphore::new(0)), events }
    }

    fn complete_commit(&self) {
        self.commits.add_permits(1);
    }
}

#[async_trait]
impl Conductor for BlockingConductor {
    async fn leader(&self) -> Result<bool, ConductorError> {
        Ok(true)
    }

    async fn active(&self) -> Result<bool, ConductorError> {
        Ok(true)
    }

    async fn commit_unsafe_payload(
        &self,
        _payload: &BaseExecutionPayloadEnvelope,
    ) -> Result<(), ConductorError> {
        self.events.send(SequencerLoopEvent::CommitStarted).expect("test event receiver is open");
        let permit =
            Arc::clone(&self.commits).acquire_owned().await.expect("commit semaphore is open");
        drop(permit);
        self.events.send(SequencerLoopEvent::CommitFinished).expect("test event receiver is open");
        Ok(())
    }

    async fn override_leader(&self) -> Result<(), ConductorError> {
        Ok(())
    }
}

#[derive(Debug)]
struct RecordingGossip {
    events: mpsc::UnboundedSender<SequencerLoopEvent>,
}

#[async_trait]
impl UnsafePayloadGossipClient for RecordingGossip {
    async fn schedule_execution_payload_gossip(
        &self,
        _payload: BaseExecutionPayloadEnvelope,
    ) -> Result<(), UnsafePayloadGossipClientError> {
        self.events.send(SequencerLoopEvent::GossipScheduled).expect("test event receiver is open");
        Ok(())
    }
}

fn loop_actor(
    runtime: ManualSequencerRuntime,
    engine_client: Arc<SequencerLoopEngine>,
    admin_api_rx: mpsc::Receiver<SequencerAdminQuery>,
    conductor: Option<BlockingConductor>,
    gossip: RecordingGossip,
) -> SequencerActor<
    TestAttributesBuilder,
    BlockingConductor,
    StaticOriginSelector,
    SequencerLoopEngine,
    RecordingGossip,
> {
    let rollup_config = Arc::new(base_common_genesis::RollupConfig::default());
    let recovery_mode = RecoveryModeGuard::new(false);
    SequencerActor {
        admin_api_rx,
        builder: PayloadBuilder {
            attributes_builder: TestAttributesBuilder {
                attributes: vec![Ok(BasePayloadAttributes::default())],
            },
            engine_client: Arc::clone(&engine_client),
            origin_selector: StaticOriginSelector,
            recovery_mode: recovery_mode.clone(),
            rollup_config: Arc::clone(&rollup_config),
        },
        cancellation_token: CancellationToken::new(),
        conductor,
        engine_client,
        is_active: true,
        recovery_mode,
        rollup_config,
        runtime: Arc::new(runtime),
        unsafe_payload_gossip_client: gossip,
        sealer: None,
        pending_stop: None,
        next_build_parent: None,
    }
}

async fn expect_loop_event(
    events: &mut mpsc::UnboundedReceiver<SequencerLoopEvent>,
    expected: SequencerLoopEvent,
) {
    let event = timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("timed out waiting for sequencer loop event")
        .expect("sequencer loop event sender is open");
    assert_eq!(event, expected);
}

#[tokio::test]
async fn test_initial_reset_services_admin_query_during_backoff_sleep() {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let runtime = ManualSequencerRuntime::new();
    let head = head_at_with_hash(1, B256::from([0x11; 32]));
    let engine_client = Arc::new(SequencerLoopEngine::new(
        event_tx.clone(),
        head,
        vec![Err(EngineClientError::ELSyncing)],
    ));
    let (admin_tx, admin_rx) = mpsc::channel(4);
    let actor =
        loop_actor(runtime, engine_client, admin_rx, None, RecordingGossip { events: event_tx });
    let cancellation = actor.cancellation_token.clone();
    let actor_task = tokio::spawn(actor.start(()));

    expect_loop_event(&mut event_rx, SequencerLoopEvent::Reset).await;

    let (response_tx, response_rx) = oneshot::channel();
    admin_tx
        .send(SequencerAdminQuery::SequencerActive(response_tx))
        .await
        .expect("sequencer admin channel is open");
    let response = timeout(Duration::from_secs(1), response_rx)
        .await
        .expect("sequencer active response timed out")
        .expect("sequencer active response sender is open")
        .expect("sequencer active query succeeds");
    assert!(response);

    cancellation.cancel();
    let result = timeout(Duration::from_secs(1), actor_task)
        .await
        .expect("sequencer actor did not stop")
        .expect("sequencer actor task did not panic");
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_stop_sequencer_during_in_flight_seal_defers_until_insert_completes() {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let runtime = ManualSequencerRuntime::new();
    let head_hash = B256::from([0x22; 32]);
    let head = head_at_with_hash(1, head_hash);
    let engine_client = Arc::new(SequencerLoopEngine::new(event_tx.clone(), head, vec![Ok(())]));
    let conductor = BlockingConductor::new(event_tx.clone());
    let (admin_tx, admin_rx) = mpsc::channel(4);
    let actor = loop_actor(
        runtime.clone(),
        engine_client,
        admin_rx,
        Some(conductor.clone()),
        RecordingGossip { events: event_tx },
    );
    let cancellation = actor.cancellation_token.clone();
    let actor_task = tokio::spawn(actor.start(()));

    expect_loop_event(&mut event_rx, SequencerLoopEvent::Reset).await;
    runtime.tick();
    expect_loop_event(&mut event_rx, SequencerLoopEvent::StartBuild).await;
    runtime.tick();
    expect_loop_event(&mut event_rx, SequencerLoopEvent::GetSealedPayload).await;
    expect_loop_event(&mut event_rx, SequencerLoopEvent::CommitStarted).await;

    let (stop_tx, mut stop_rx) = oneshot::channel();
    admin_tx
        .send(SequencerAdminQuery::StopSequencer(stop_tx))
        .await
        .expect("sequencer admin channel is open");
    let (active_tx, active_rx) = oneshot::channel();
    admin_tx
        .send(SequencerAdminQuery::SequencerActive(active_tx))
        .await
        .expect("sequencer admin channel is open");
    let active = timeout(Duration::from_secs(1), active_rx)
        .await
        .expect("sequencer active response timed out")
        .expect("sequencer active response sender is open")
        .expect("sequencer active query succeeds");
    assert!(!active);
    assert!(matches!(stop_rx.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));

    conductor.complete_commit();

    expect_loop_event(&mut event_rx, SequencerLoopEvent::CommitStarted).await;
    expect_loop_event(&mut event_rx, SequencerLoopEvent::CommitFinished).await;
    expect_loop_event(&mut event_rx, SequencerLoopEvent::GossipScheduled).await;
    expect_loop_event(&mut event_rx, SequencerLoopEvent::InsertUnsafePayload).await;
    let stop_head = timeout(Duration::from_secs(1), stop_rx)
        .await
        .expect("stop_sequencer response timed out")
        .expect("stop_sequencer response sender is open")
        .expect("stop_sequencer succeeds");
    assert_eq!(stop_head, head_hash);

    cancellation.cancel();
    let result = timeout(Duration::from_secs(1), actor_task)
        .await
        .expect("sequencer actor did not stop")
        .expect("sequencer actor task did not panic");
    assert!(result.is_ok());
}

// --- try_seal_handle tests ---

#[tokio::test]
async fn test_try_seal_handle_current_head_equals_parent_seals() {
    // head.number == parent.number AND head.hash == parent.hash → not stale; seal proceeds.
    // Use a distinct non-zero hash so the hash equality check is actually exercised.
    let hash = B256::from([0xcc; 32]);

    let mut client = MockSequencerEngineClient::new();
    client.expect_get_unsafe_head().times(1).return_once(move || Ok(head_at_with_hash(5, hash)));
    client.expect_get_sealed_payload().times(1).return_once(|_, _| Ok(dummy_envelope()));

    let mut actor = test_actor();
    actor.engine_client = Arc::new(client);

    let (sealer, dur) = actor.try_seal_handle(handle_with_parent(5, hash)).await.unwrap().unwrap();
    assert_eq!(sealer.state, SealState::Sealed);
    assert!(dur < Duration::from_secs(10));
}

#[tokio::test]
async fn test_try_seal_handle_current_head_ahead_of_parent_discards() {
    // head > parent → stale; seal_payload must NOT be called.
    let mut client = MockSequencerEngineClient::new();
    client.expect_get_unsafe_head().times(1).return_once(|| Ok(head_at(6)));
    client.expect_get_sealed_payload().times(0);

    let mut actor = test_actor();
    actor.engine_client = Arc::new(client);

    let result = actor.try_seal_handle(handle_with_parent_number(5)).await;

    assert!(result.unwrap().is_none());
}

#[tokio::test]
async fn test_try_seal_handle_same_height_reorg_discards() {
    // head.number == parent.number but head.hash != parent.hash → same-height reorg; discard.
    let parent_hash = B256::from([0xaa; 32]);
    let reorged_hash = B256::from([0xbb; 32]);

    let mut client = MockSequencerEngineClient::new();
    client
        .expect_get_unsafe_head()
        .times(1)
        .return_once(move || Ok(head_at_with_hash(5, reorged_hash)));
    client.expect_get_sealed_payload().times(0);

    let mut actor = test_actor();
    actor.engine_client = Arc::new(client);

    let result = actor.try_seal_handle(handle_with_parent(5, parent_hash)).await;

    assert!(result.unwrap().is_none());
}

#[tokio::test]
async fn test_try_seal_handle_get_unsafe_head_error_propagates() {
    let mut client = MockSequencerEngineClient::new();
    client
        .expect_get_unsafe_head()
        .times(1)
        .return_once(|| Err(EngineClientError::RequestError("channel closed".to_string())));
    client.expect_get_sealed_payload().times(0);

    let mut actor = test_actor();
    actor.engine_client = Arc::new(client);

    let result = actor.try_seal_handle(handle_with_parent_number(5)).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_try_seal_handle_fatal_seal_error_cancels_and_propagates() {
    // A fatal seal error must cancel the token and return Err.
    let mut client = MockSequencerEngineClient::new();
    client.expect_get_unsafe_head().times(1).return_once(|| Ok(head_at(5)));
    client.expect_get_sealed_payload().times(1).return_once(|_, _| {
        Err(EngineClientError::SealError(SealTaskError::DepositOnlyPayloadFailed))
    });

    let mut actor = test_actor();
    actor.engine_client = Arc::new(client);

    let result = actor.try_seal_handle(handle_with_parent_number(5)).await;

    assert!(result.is_err());
    assert!(actor.cancellation_token.is_cancelled());
}

#[tokio::test]
async fn test_try_seal_handle_non_fatal_seal_error_returns_none() {
    // A non-fatal seal error must return Ok(None) and leave the token uncancelled.
    let mut client = MockSequencerEngineClient::new();
    client.expect_get_unsafe_head().times(1).return_once(|| Ok(head_at(5)));
    client
        .expect_get_sealed_payload()
        .times(1)
        .return_once(|_, _| Err(EngineClientError::SealError(SealTaskError::HoloceneInvalidFlush)));

    let mut actor = test_actor();
    actor.engine_client = Arc::new(client);

    let result = actor.try_seal_handle(handle_with_parent_number(5)).await;

    assert!(result.unwrap().is_none());
    assert!(!actor.cancellation_token.is_cancelled());
}

// --- build tests ---

#[rstest]
#[case::temp(PipelineErrorKind::Temporary(BuilderError::Custom(String::new()).into()), false)]
#[case::reset(PipelineErrorKind::Reset(BuilderError::Custom(String::new()).into()), false)]
#[case::critical(PipelineErrorKind::Critical(BuilderError::Custom(String::new()).into()), true)]
#[tokio::test]
async fn test_build_unsealed_payload_prepare_payload_attributes_error(
    #[case] forced_error: PipelineErrorKind,
    #[case] expect_err: bool,
) {
    let mut client = MockSequencerEngineClient::new();

    let unsafe_head = L2BlockInfo::default();
    client.expect_get_unsafe_head().times(1).return_once(move || Ok(unsafe_head));
    client.expect_start_build_block().times(0);
    // Reset pipeline errors no longer trigger engine reset — the attributes builder is stateless
    // so resetting the engine would only rewind the unsafe head without aiding recovery.
    client.expect_reset_engine_forkchoice().times(0);

    let l1_origin = BlockInfo::default();
    let mut origin_selector = MockOriginSelector::new();
    origin_selector.expect_next_l1_origin().times(1).return_once(move |_, _| Ok(l1_origin));

    let attributes_builder = TestAttributesBuilder { attributes: vec![Err(forced_error)] };

    let mut actor = test_actor();
    actor.builder.origin_selector = origin_selector;
    actor.builder.engine_client = Arc::new(client);
    actor.builder.attributes_builder = attributes_builder;

    let result = actor.builder.build().await;
    if expect_err {
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SequencerActorError::AttributesBuilder(PipelineErrorKind::Critical(_))
        ));
    } else {
        assert!(result.is_ok());
    }
}

// --- seal_payload tests ---

#[tokio::test]
async fn test_seal_payload_success_returns_sealer() {
    let envelope = dummy_envelope();

    let mut client = MockSequencerEngineClient::new();
    client.expect_get_sealed_payload().times(1).return_once(move |_, _| Ok(envelope));

    let mut actor = test_actor();
    actor.engine_client = Arc::new(client);

    let handle = UnsealedPayloadHandle {
        payload_id: Default::default(),
        attributes_with_parent: dummy_attributes_with_parent(),
    };
    let sealer = actor.seal_payload(&handle).await;

    assert!(sealer.is_ok());
    assert_eq!(sealer.unwrap().state, SealState::Sealed);
}

#[tokio::test]
async fn test_seal_payload_failure_propagates() {
    let mut client = MockSequencerEngineClient::new();
    client
        .expect_get_sealed_payload()
        .times(1)
        .return_once(|_, _| Err(EngineClientError::RequestError("engine offline".to_string())));

    let mut actor = test_actor();
    actor.engine_client = Arc::new(client);

    let handle = UnsealedPayloadHandle {
        payload_id: Default::default(),
        attributes_with_parent: dummy_attributes_with_parent(),
    };
    let result = actor.seal_payload(&handle).await;

    assert!(result.is_err());
}

// --- PayloadSealer::step tests ---

#[tokio::test]
async fn test_sealer_full_pipeline_no_conductor() {
    let envelope = dummy_envelope();

    let mut gossip = MockUnsafePayloadGossipClient::new();
    gossip.expect_schedule_execution_payload_gossip().times(1).return_once(|_| Ok(()));

    let mut engine = MockSequencerEngineClient::new();
    engine.expect_insert_unsafe_payload().times(1).return_once(|_| Ok(()));

    let conductor: Option<MockConductor> = None;
    let mut sealer = PayloadSealer::new(envelope);

    assert_eq!(sealer.state, SealState::Sealed);

    let result = sealer.step(&conductor, &gossip, &engine).await;
    assert!(!result.unwrap());
    assert_eq!(sealer.state, SealState::Committed);

    let result = sealer.step(&conductor, &gossip, &engine).await;
    assert!(!result.unwrap());
    assert_eq!(sealer.state, SealState::Gossiped);

    let result = sealer.step(&conductor, &gossip, &engine).await;
    assert!(result.unwrap());
}

#[tokio::test]
async fn test_sealer_full_pipeline_with_conductor() {
    let envelope = dummy_envelope();

    let mut conductor = MockConductor::new();
    conductor.expect_commit_unsafe_payload().times(1).return_once(|_| Ok(()));

    let mut gossip = MockUnsafePayloadGossipClient::new();
    gossip.expect_schedule_execution_payload_gossip().times(1).return_once(|_| Ok(()));

    let mut engine = MockSequencerEngineClient::new();
    engine.expect_insert_unsafe_payload().times(1).return_once(|_| Ok(()));

    let conductor = Some(conductor);
    let mut sealer = PayloadSealer::new(envelope);

    let result = sealer.step(&conductor, &gossip, &engine).await;
    assert!(!result.unwrap());
    assert_eq!(sealer.state, SealState::Committed);

    let result = sealer.step(&conductor, &gossip, &engine).await;
    assert!(!result.unwrap());
    assert_eq!(sealer.state, SealState::Gossiped);

    let result = sealer.step(&conductor, &gossip, &engine).await;
    assert!(result.unwrap());
}

#[tokio::test]
async fn test_sealer_conductor_failure_stays_sealed() {
    let envelope = dummy_envelope();

    let mut conductor = MockConductor::new();
    conductor.expect_commit_unsafe_payload().times(1).return_once(|_| Err(conductor_rpc_error()));

    let gossip = MockUnsafePayloadGossipClient::new();
    let engine = MockSequencerEngineClient::new();

    let conductor = Some(conductor);
    let mut sealer = PayloadSealer::new(envelope);

    let result = sealer.step(&conductor, &gossip, &engine).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), SealStepError::Conductor(_)));
    assert_eq!(sealer.state, SealState::Sealed);
}

#[tokio::test]
async fn test_sealer_gossip_failure_stays_committed() {
    let envelope = dummy_envelope();

    let mut gossip = MockUnsafePayloadGossipClient::new();
    gossip.expect_schedule_execution_payload_gossip().times(1).return_once(|_| {
        Err(UnsafePayloadGossipClientError::RequestError("channel closed".to_string()))
    });

    let engine = MockSequencerEngineClient::new();
    let conductor: Option<MockConductor> = None;
    let mut sealer = PayloadSealer::new(envelope);

    let _ = sealer.step(&conductor, &gossip, &engine).await.unwrap();
    assert_eq!(sealer.state, SealState::Committed);

    let result = sealer.step(&conductor, &gossip, &engine).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), SealStepError::Gossip(_)));
    assert_eq!(sealer.state, SealState::Committed);
}

#[tokio::test]
async fn test_sealer_insert_failure_stays_gossiped() {
    let envelope = dummy_envelope();

    let mut gossip = MockUnsafePayloadGossipClient::new();
    gossip.expect_schedule_execution_payload_gossip().times(1).return_once(|_| Ok(()));

    let mut engine = MockSequencerEngineClient::new();
    engine
        .expect_insert_unsafe_payload()
        .times(1)
        .return_once(|_| Err(EngineClientError::RequestError("channel closed".to_string())));

    let conductor: Option<MockConductor> = None;
    let mut sealer = PayloadSealer::new(envelope);

    let _ = sealer.step(&conductor, &gossip, &engine).await.unwrap();
    let _ = sealer.step(&conductor, &gossip, &engine).await.unwrap();
    assert_eq!(sealer.state, SealState::Gossiped);

    let result = sealer.step(&conductor, &gossip, &engine).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), SealStepError::Insert(_)));
    assert_eq!(sealer.state, SealState::Gossiped);
}
