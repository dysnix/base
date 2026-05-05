//! Tests for actual precompile contract storage layouts.
//!
//! This module verifies that the storage layouts of production precompile contracts
//! match their Solidity equivalents, ensuring compatibility with the EVM.

use super::*;
use base_precompiles_macros::{
    gen_test_fields_layout as layout_fields, gen_test_fields_struct as struct_fields,
};
use utils::*;

#[test]
fn test_b403_registry_layout() {
    use base_precompiles::b403_registry::{__packing_policy_record::*, slots};

    let sol_path = testdata("b403_registry.sol");
    let solc_layout = load_solc_layout(&sol_path);

    // Verify top-level fields
    let rust_layout = layout_fields!(policy_id_counter, policy_records, policy_set);
    if let Err(errors) = compare_layouts(&solc_layout, &rust_layout) {
        panic_layout_mismatch("Layout", errors, &sol_path);
    }

    // Verify `PolicyRecord` struct members (nested under policyRecords mapping)
    let base_slot = slots::POLICY_RECORDS;
    let rust_policy_record = struct_fields!(base_slot, base, compound);
    if let Err(errors) = compare_struct_members(&solc_layout, "policyRecords", &rust_policy_record)
    {
        panic_layout_mismatch("PolicyRecord struct layout", errors, &sol_path);
    }

    // Verify `PolicyData` struct members (nested in PolicyRecord.base)
    {
        use base_precompiles::b403_registry::__packing_policy_data::*;
        let rust_policy_data = struct_fields!(base_slot, policy_type, admin);
        if let Err(errors) =
            compare_nested_struct_type(&solc_layout, "PolicyData", &rust_policy_data)
        {
            panic_layout_mismatch("PolicyData struct layout", errors, &sol_path);
        }
    }

    // Verify `CompoundPolicyData` struct members (nested in PolicyRecord.compound)
    {
        use base_precompiles::b403_registry::__packing_compound_policy_data::*;
        let rust_compound = struct_fields!(
            base_slot,
            sender_policy_id,
            recipient_policy_id,
            mint_recipient_policy_id
        );
        if let Err(errors) =
            compare_nested_struct_type(&solc_layout, "CompoundPolicyData", &rust_compound)
        {
            panic_layout_mismatch("CompoundPolicyData struct layout", errors, &sol_path);
        }
    }
}

#[test]
fn test_b20_layout() {
    use base_precompiles::b20::{rewards::__packing_user_reward_info::*, slots};

    let sol_path = testdata("b20.sol");
    let solc_layout = load_solc_layout(&sol_path);

    // Verify top-level fields
    let rust_layout = layout_fields!(
        // RolesAuth
        roles,
        role_admins,
        // B20 Metadata
        name,
        symbol,
        currency,
        // Unused slot, kept for storage layout compatibility
        _domain_separator,
        quote_token,
        next_quote_token,
        transfer_policy_id,
        // B20 Token
        total_supply,
        balances,
        allowances,
        // EIP-2612 permit nonces (B1004)
        permit_nonces,
        paused,
        supply_cap,
        // Unused slot, kept for storage layout compatibility
        _salts,
        // B20 Rewards
        global_reward_per_token,
        opted_in_supply,
        user_reward_info
    );
    if let Err(errors) = compare_layouts(&solc_layout, &rust_layout) {
        panic_layout_mismatch("Layout", errors, &sol_path);
    }

    // Verify `UserRewardInfo` struct members
    let user_info_base_slot = slots::USER_REWARD_INFO;
    let rust_user_info =
        struct_fields!(user_info_base_slot, reward_recipient, reward_per_token, reward_balance);
    if let Err(errors) = compare_struct_members(&solc_layout, "userRewardInfo", &rust_user_info) {
        panic_layout_mismatch("UserRewardInfo struct member layout", errors, &sol_path);
    }
}

/// Export all storage constants to a JSON file for comparison between branches.
///
/// This test is marked with #[ignore] so it doesn't run in CI.
/// To run it manually:
/// ```bash
/// cargo test export_all_storage_constants -- --ignored --nocapture
/// ```
///
/// The output will be written to `current_branch_constants.json` in the workspace root.
#[test]
#[ignore]
fn export_all_storage_constants() {
    use serde_json::json;
    use std::fs;

    let mut all_constants = serde_json::Map::new();

    // Helper to convert RustStorageField to JSON
    let field_to_json = |field: &utils::RustStorageField| {
        json!({
            "name": field.name,
            "slot": format!("{:#x}", field.slot),
            "offset": field.offset,
            "bytes": field.bytes
        })
    };

    // B403 Registry
    {
        use base_precompiles::b403_registry::{__packing_policy_record::*, slots};

        let fields = layout_fields!(policy_id_counter, policy_records, policy_set);
        let base_slot = slots::POLICY_RECORDS;
        let policy_record_struct = struct_fields!(base_slot, base, compound);

        let policy_data_struct = {
            use base_precompiles::b403_registry::__packing_policy_data::*;
            struct_fields!(base_slot, policy_type, admin)
        };

        let compound_policy_data_struct = {
            use base_precompiles::b403_registry::__packing_compound_policy_data::*;
            struct_fields!(
                base_slot,
                sender_policy_id,
                recipient_policy_id,
                mint_recipient_policy_id
            )
        };

        all_constants.insert(
            "b403_registry".to_string(),
            json!({
                "fields": fields.iter().map(field_to_json).collect::<Vec<_>>(),
                "structs": {
                    "policyRecords": policy_record_struct.iter().map(field_to_json).collect::<Vec<_>>(),
                    "policyData": policy_data_struct.iter().map(field_to_json).collect::<Vec<_>>(),
                    "compoundPolicyData": compound_policy_data_struct.iter().map(field_to_json).collect::<Vec<_>>()
                }
            }),
        );
    }

    // B20 Token
    {
        use base_precompiles::b20::{rewards::__packing_user_reward_info::*, slots};

        let fields = layout_fields!(
            // RolesAuth
            roles,
            role_admins,
            // B20 Metadata
            name,
            symbol,
            currency,
            // Unused slot, kept for storage layout compatibility
            _domain_separator,
            quote_token,
            next_quote_token,
            transfer_policy_id,
            // B20 Token
            total_supply,
            balances,
            allowances,
            // EIP-2612 permit nonces (B1004)
            permit_nonces,
            paused,
            supply_cap,
            // Unused slot, kept for storage layout compatibility
            _salts,
            // B20 Rewards
            global_reward_per_token,
            opted_in_supply,
            user_reward_info
        );

        let user_info_base_slot = slots::USER_REWARD_INFO;
        let user_info_struct =
            struct_fields!(user_info_base_slot, reward_recipient, reward_per_token, reward_balance);

        all_constants.insert(
            "b20".to_string(),
            json!({
                "fields": fields.iter().map(field_to_json).collect::<Vec<_>>(),
                "structs": {
                    "userRewardInfo": user_info_struct.iter().map(field_to_json).collect::<Vec<_>>()
                }
            }),
        );
    }

    // Write to file
    let output = serde_json::to_string_pretty(&all_constants).unwrap();
    let current_dir = std::env::current_dir().unwrap();
    let workspace_root = current_dir
        .ancestors()
        .find(|p| p.join("Cargo.toml").exists() && p.join("crates").exists())
        .expect("Could not find workspace root");

    let output_path = workspace_root.join("current_branch_constants.json");
    fs::write(&output_path, output).unwrap();

    println!("✅ Storage constants exported to: {}", output_path.display());
}
