// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import { Test } from "forge-std/Test.sol";

import { IDisputeGame } from "interfaces/dispute/IDisputeGame.sol";
import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { INitroEnclaveVerifier } from "interfaces/multiproof/tee/INitroEnclaveVerifier.sol";
import {
    TDXTcbStatus,
    TDXVerificationResult,
    TDXVerifierJournal
} from "interfaces/multiproof/tee/ITDXVerifier.sol";
import { ZkCoProcessorConfig, ZkCoProcessorType } from "interfaces/multiproof/tee/INitroEnclaveVerifier.sol";
import { GameType } from "src/dispute/lib/Types.sol";
import { TDXVerifier } from "src/multiproof/tee/TDXVerifier.sol";

interface ITDXEndToEndRegistry {
    function registerTDXSigner(bytes calldata output, bytes calldata proofBytes) external;

    function isRegisteredSigner(address signer) external view returns (bool);

    function signerImageHash(address signer) external view returns (bytes32);

    function isValidSigner(address signer) external view returns (bool);
}

contract TDXEndToEndAggregateVerifier {
    bytes32 public TEE_IMAGE_HASH;

    constructor(bytes32 imageHash) {
        TEE_IMAGE_HASH = imageHash;
    }
}

contract TDXEndToEndDisputeGameFactory {
    mapping(uint32 => address) internal _impls;

    function setImpl(uint32 gameType, address impl) external {
        _impls[gameType] = impl;
    }

    function gameImpls(GameType gameType) external view returns (IDisputeGame) {
        return IDisputeGame(_impls[GameType.unwrap(gameType)]);
    }
}

contract TDXEndToEndRegistrationTest is Test {
    bytes32 internal constant ROOT_CA_HASH = keccak256("intel-root-ca");
    bytes32 internal constant VERIFIER_ID = keccak256("tdx-verifier-id");
    bytes32 internal constant AGGREGATOR_ID = keccak256("tdx-aggregator-id");
    bytes32 internal constant IMAGE_HASH = keccak256("tdx-image");
    bytes32 internal constant MRTD_HASH = keccak256("mrtd");
    bytes32 internal constant REPORT_DATA_SUFFIX = keccak256("base-tdx-tee-prover-v1");

    uint64 internal constant MAX_TIME_DIFF = 3600;
    uint256 internal constant NOW = 1_700_000_000;

    address internal mockRiscZeroVerifier;
    TDXVerifier internal tdxVerifier;
    ITDXEndToEndRegistry internal registry;

    function setUp() public {
        vm.warp(NOW);
        mockRiscZeroVerifier = makeAddr("mock-risc-zero");

        TDXTcbStatus[] memory allowedStatuses = new TDXTcbStatus[](1);
        allowedStatuses[0] = TDXTcbStatus.UpToDate;

        tdxVerifier = new TDXVerifier(
            address(this),
            MAX_TIME_DIFF,
            ROOT_CA_HASH,
            address(this),
            ZkCoProcessorType.RiscZero,
            ZkCoProcessorConfig({
                verifierId: VERIFIER_ID,
                aggregatorId: AGGREGATOR_ID,
                zkVerifier: mockRiscZeroVerifier
            }),
            allowedStatuses
        );

        TDXEndToEndDisputeGameFactory factory = new TDXEndToEndDisputeGameFactory();
        factory.setImpl(0, address(new TDXEndToEndAggregateVerifier(IMAGE_HASH)));
        registry = ITDXEndToEndRegistry(
            deployCode(
                "src/multiproof/tee/TDXTEEProverRegistry.sol:TDXTEEProverRegistry",
                abi.encode(
                    INitroEnclaveVerifier(address(0)),
                    tdxVerifier,
                    IDisputeGameFactory(address(factory))
                )
            )
        );
        tdxVerifier.setProofSubmitter(address(registry));
    }

    function testRegistersTdxSignerThroughVerifierAndRegistry() public {
        TDXVerifierJournal memory journal = _successJournal();
        bytes memory output = abi.encode(journal);
        bytes memory proofBytes = hex"1234";
        _mockRiscZeroVerify(output, proofBytes);

        vm.prank(address(0xdEaD));
        registry.registerTDXSigner(output, proofBytes);

        assertTrue(registry.isRegisteredSigner(journal.signer));
        assertEq(registry.signerImageHash(journal.signer), IMAGE_HASH);
        assertTrue(registry.isValidSigner(journal.signer));
    }

    function _successJournal() internal view returns (TDXVerifierJournal memory journal) {
        bytes memory publicKey = _publicKey();
        bytes32 publicKeyHash;
        assembly {
            publicKeyHash := keccak256(add(publicKey, 0x21), 64)
        }

        journal = TDXVerifierJournal({
            result: TDXVerificationResult.Success,
            tcbStatus: TDXTcbStatus.UpToDate,
            timestamp: uint64(block.timestamp - 1) * 1000,
            collateralExpiration: uint64(block.timestamp + 1 days),
            rootCaHash: ROOT_CA_HASH,
            pckCertHash: keccak256("pck-cert"),
            tcbInfoHash: keccak256("tcb-info"),
            qeIdentityHash: keccak256("qe-identity"),
            publicKey: publicKey,
            signer: address(uint160(uint256(publicKeyHash))),
            imageHash: IMAGE_HASH,
            mrTdHash: MRTD_HASH,
            reportDataPrefix: publicKeyHash,
            reportDataSuffix: REPORT_DATA_SUFFIX
        });
    }

    function _publicKey() internal pure returns (bytes memory publicKey) {
        publicKey = new bytes(65);
        publicKey[0] = 0x04;
        for (uint256 i = 1; i < publicKey.length; i++) {
            publicKey[i] = bytes1(uint8(i));
        }
    }

    function _mockRiscZeroVerify(bytes memory output, bytes memory proofBytes) internal {
        vm.mockCall(
            mockRiscZeroVerifier,
            abi.encodeWithSelector(
                bytes4(keccak256("verify(bytes,bytes32,bytes32)")),
                proofBytes,
                VERIFIER_ID,
                sha256(output)
            ),
            ""
        );
    }
}
