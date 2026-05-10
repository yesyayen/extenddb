// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0

//! `ManagementStore` implementation for `PostgresCatalogStore`.
//!
//! The trait impl delegates to `_impl` methods in submodules, keeping each
//! file under the 500-line limit.

use extenddb_storage::management_store::{
    AccessKeyCreated, AccountDetail, GroupDetail, OpResult, RoleDetail, UserDetail,
};

use super::catalog_store::PostgresCatalogStore;

mod access_keys;
mod accounts;
mod groups;
mod policies;
mod roles;
mod users;

impl extenddb_storage::management_store::ManagementStore for PostgresCatalogStore {
    // ── Accounts ───────────────────────────────────────────────────

    async fn create_account(&self, account_id: &str, account_name: &str) -> OpResult<()> {
        self.create_account_impl(account_id, account_name).await
    }

    async fn delete_account(&self, account_id: &str) -> OpResult<()> {
        self.delete_account_impl(account_id).await
    }

    async fn list_all_accounts(&self) -> OpResult<Vec<(String, String)>> {
        self.list_all_accounts_impl().await
    }

    async fn list_all_accounts_full(
        &self,
    ) -> OpResult<Vec<(String, String, time::OffsetDateTime)>> {
        self.list_all_accounts_full_impl().await
    }

    async fn list_accounts_for(&self, account_id: &str) -> OpResult<Vec<(String, String)>> {
        self.list_accounts_for_impl(account_id).await
    }

    async fn get_account_detail(&self, account_id: &str) -> OpResult<Option<AccountDetail>> {
        self.get_account_detail_impl(account_id).await
    }

    async fn dashboard_counts(&self) -> OpResult<(i64, i64)> {
        self.dashboard_counts_impl().await
    }

    // ── Users ──────────────────────────────────────────────────────

    async fn create_user(
        &self,
        account_id: &str,
        user_name: &str,
        password_hash: Option<&str>,
    ) -> OpResult<()> {
        self.create_user_impl(account_id, user_name, password_hash)
            .await
    }

    async fn delete_user(&self, account_id: &str, user_name: &str) -> OpResult<()> {
        self.delete_user_impl(account_id, user_name).await
    }

    async fn list_users(
        &self,
        account_id: &str,
    ) -> OpResult<Vec<(String, String, String, bool, time::OffsetDateTime)>> {
        self.list_users_impl(account_id).await
    }

    async fn get_user_detail(
        &self,
        account_id: &str,
        user_name: &str,
    ) -> OpResult<Option<UserDetail>> {
        self.get_user_detail_impl(account_id, user_name).await
    }

    async fn verify_iam_user_password(
        &self,
        account_id: &str,
        user_name: &str,
        password: &str,
    ) -> OpResult<bool> {
        self.verify_iam_user_password_impl(account_id, user_name, password)
            .await
    }

    async fn change_user_password(
        &self,
        account_id: &str,
        user_name: &str,
        password_hash: &str,
    ) -> OpResult<()> {
        self.change_user_password_impl(account_id, user_name, password_hash)
            .await
    }

    async fn tag_user(
        &self,
        account_id: &str,
        user_name: &str,
        tags: &[(String, String)],
    ) -> OpResult<()> {
        self.tag_user_impl(account_id, user_name, tags).await
    }

    async fn untag_user(
        &self,
        account_id: &str,
        user_name: &str,
        tag_keys: &[String],
    ) -> OpResult<()> {
        self.untag_user_impl(account_id, user_name, tag_keys).await
    }

    async fn list_user_tags(
        &self,
        account_id: &str,
        user_name: &str,
    ) -> OpResult<Vec<(String, String)>> {
        self.list_user_tags_impl(account_id, user_name).await
    }

    // ── Groups ─────────────────────────────────────────────────────

    async fn create_group(&self, account_id: &str, group_name: &str) -> OpResult<()> {
        self.create_group_impl(account_id, group_name).await
    }

    async fn delete_group(&self, account_id: &str, group_name: &str) -> OpResult<()> {
        self.delete_group_impl(account_id, group_name).await
    }

    async fn list_groups(
        &self,
        account_id: &str,
    ) -> OpResult<Vec<(String, String, String, time::OffsetDateTime)>> {
        self.list_groups_impl(account_id).await
    }

    async fn get_group_detail(
        &self,
        account_id: &str,
        group_name: &str,
    ) -> OpResult<Option<GroupDetail>> {
        self.get_group_detail_impl(account_id, group_name).await
    }

    async fn add_group_member(
        &self,
        account_id: &str,
        group_name: &str,
        user_name: &str,
    ) -> OpResult<()> {
        self.add_group_member_impl(account_id, group_name, user_name)
            .await
    }

    async fn remove_group_member(
        &self,
        account_id: &str,
        group_name: &str,
        user_name: &str,
    ) -> OpResult<()> {
        self.remove_group_member_impl(account_id, group_name, user_name)
            .await
    }

    // ── Roles ──────────────────────────────────────────────────────

    async fn create_role(
        &self,
        account_id: &str,
        role_name: &str,
        trust_policy: &serde_json::Value,
    ) -> OpResult<()> {
        self.create_role_impl(account_id, role_name, trust_policy)
            .await
    }

    async fn delete_role(&self, account_id: &str, role_name: &str) -> OpResult<()> {
        self.delete_role_impl(account_id, role_name).await
    }

    async fn list_roles(
        &self,
        account_id: &str,
    ) -> OpResult<
        Vec<(
            String,
            String,
            String,
            serde_json::Value,
            time::OffsetDateTime,
        )>,
    > {
        self.list_roles_impl(account_id).await
    }

    async fn get_role_detail(
        &self,
        account_id: &str,
        role_name: &str,
    ) -> OpResult<Option<RoleDetail>> {
        self.get_role_detail_impl(account_id, role_name).await
    }

    async fn get_role_trust_policy(
        &self,
        account_id: &str,
        role_name: &str,
    ) -> OpResult<Option<serde_json::Value>> {
        self.get_role_trust_policy_impl(account_id, role_name).await
    }

    async fn tag_role(
        &self,
        account_id: &str,
        role_name: &str,
        tags: &[(String, String)],
    ) -> OpResult<()> {
        self.tag_role_impl(account_id, role_name, tags).await
    }

    async fn untag_role(
        &self,
        account_id: &str,
        role_name: &str,
        tag_keys: &[String],
    ) -> OpResult<()> {
        self.untag_role_impl(account_id, role_name, tag_keys).await
    }

    async fn list_role_tags(
        &self,
        account_id: &str,
        role_name: &str,
    ) -> OpResult<Vec<(String, String)>> {
        self.list_role_tags_impl(account_id, role_name).await
    }

    // ── Policies ───────────────────────────────────────────────────

    async fn put_policy(
        &self,
        account_id: &str,
        principal_type: &str,
        principal_name: &str,
        policy_name: &str,
        document: &serde_json::Value,
    ) -> OpResult<()> {
        self.put_policy_impl(
            account_id,
            principal_type,
            principal_name,
            policy_name,
            document,
        )
        .await
    }

    async fn delete_policy(
        &self,
        account_id: &str,
        principal_type: &str,
        principal_name: &str,
        policy_name: &str,
    ) -> OpResult<()> {
        self.delete_policy_impl(account_id, principal_type, principal_name, policy_name)
            .await
    }

    async fn list_policies(
        &self,
        account_id: &str,
        principal_type: &str,
        principal_name: &str,
    ) -> OpResult<Vec<(String, serde_json::Value, time::OffsetDateTime)>> {
        self.list_policies_impl(account_id, principal_type, principal_name)
            .await
    }

    // ── Permissions boundaries ─────────────────────────────────────

    async fn set_user_boundary(
        &self,
        account_id: &str,
        user_name: &str,
        document: &serde_json::Value,
    ) -> OpResult<()> {
        self.set_boundary_impl(account_id, "user", user_name, document)
            .await
    }

    async fn get_user_boundary(
        &self,
        account_id: &str,
        user_name: &str,
    ) -> OpResult<Option<serde_json::Value>> {
        self.get_boundary_impl(account_id, "user", user_name).await
    }

    async fn delete_user_boundary(&self, account_id: &str, user_name: &str) -> OpResult<()> {
        self.delete_boundary_impl(account_id, "user", user_name)
            .await
    }

    async fn set_role_boundary(
        &self,
        account_id: &str,
        role_name: &str,
        document: &serde_json::Value,
    ) -> OpResult<()> {
        self.set_boundary_impl(account_id, "role", role_name, document)
            .await
    }

    async fn get_role_boundary(
        &self,
        account_id: &str,
        role_name: &str,
    ) -> OpResult<Option<serde_json::Value>> {
        self.get_boundary_impl(account_id, "role", role_name).await
    }

    async fn delete_role_boundary(&self, account_id: &str, role_name: &str) -> OpResult<()> {
        self.delete_boundary_impl(account_id, "role", role_name)
            .await
    }

    // ── Access keys ────────────────────────────────────────────────

    async fn create_access_key(
        &self,
        account_id: &str,
        user_name: &str,
    ) -> OpResult<AccessKeyCreated> {
        self.create_access_key_impl(account_id, user_name).await
    }

    async fn delete_access_key(
        &self,
        account_id: &str,
        user_name: &str,
        key_id: &str,
    ) -> OpResult<()> {
        self.delete_access_key_impl(account_id, user_name, key_id)
            .await
    }

    async fn list_access_keys(
        &self,
        account_id: &str,
        user_name: &str,
    ) -> OpResult<Vec<(String, bool, time::OffsetDateTime)>> {
        self.list_access_keys_impl(account_id, user_name).await
    }

    async fn import_access_key(
        &self,
        account_id: &str,
        user_name: &str,
        access_key_id: &str,
        secret_access_key: &str,
    ) -> OpResult<()> {
        self.import_access_key_impl(account_id, user_name, access_key_id, secret_access_key)
            .await
    }

    // ── Sessions ───────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    async fn store_session(
        &self,
        session_token: &str,
        access_key_id: &str,
        secret_key_encrypted: &[u8],
        account_id: &str,
        role_name: &str,
        session_name: &str,
        session_tags: &Option<serde_json::Value>,
        session_policy: &Option<serde_json::Value>,
        expires_at: time::OffsetDateTime,
    ) -> OpResult<()> {
        self.store_session_impl(
            session_token,
            access_key_id,
            secret_key_encrypted,
            account_id,
            role_name,
            session_name,
            session_tags,
            session_policy,
            expires_at,
        )
        .await
    }

    // ── Caller tags ────────────────────────────────────────────────

    async fn fetch_caller_tags(
        &self,
        account_id: &str,
        resource: &str,
    ) -> OpResult<Vec<(String, String)>> {
        self.fetch_caller_tags_impl(account_id, resource).await
    }
}
