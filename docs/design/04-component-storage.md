# extenddb — Component Design: Storage

**Version:** 1.0
**Date:** 2026-04-03
**Status:** Draft
**Crates:** `dynamodb-storage` (trait), `dynamodb-storage-postgres` (PostgreSQL backend)

## 1. Purpose

The storage layer provides a clean async trait that abstracts all persistent data operations. The trait is defined in the `dynamodb-storage` crate (no database dependencies). Backend implementations live in separate crates (e.g., `dynamodb-storage-postgres`).

## 2. StorageEngine Traits

The storage abstraction is split into focused sub-traits following the Interface Segregation Principle. This allows new backends to implement only what they need, and allows consumers (e.g., the auth crate) to depend on only the sub-trait they require.

```rust
use dynamodb_core::types::*;
use dynamodb_core::expression::ast::Expr;
use std::collections::BTreeMap;

pub type Item = BTreeMap<String, AttributeValue>;

/// Table lifecycle operations.
pub trait TableEngine: Send + Sync {
    fn create_table(&self, input: CreateTableInput) -> impl Future<Output = Result<TableMetadata, StorageError>> + Send;
    fn delete_table(&self, table_name: &str) -> impl Future<Output = Result<TableMetadata, StorageError>> + Send;
    fn describe_table(&self, table_name: &str) -> impl Future<Output = Result<TableMetadata, StorageError>> + Send;
    fn update_table(&self, input: UpdateTableInput) -> impl Future<Output = Result<TableMetadata, StorageError>> + Send;
    fn list_tables(&self, input: ListTablesInput) -> impl Future<Output = Result<ListTablesOutput, StorageError>> + Send;
}

/// Item CRUD, query, scan, batch, and transaction operations.
pub trait DataEngine: Send + Sync {
    fn put_item(&self, input: PutItemInput) -> impl Future<Output = Result<PutItemOutput, StorageError>> + Send;
    fn get_item(&self, input: GetItemInput) -> impl Future<Output = Result<GetItemOutput, StorageError>> + Send;
    fn delete_item(&self, input: DeleteItemInput) -> impl Future<Output = Result<DeleteItemOutput, StorageError>> + Send;
    fn update_item(&self, input: UpdateItemInput) -> impl Future<Output = Result<UpdateItemOutput, StorageError>> + Send;

    fn query(&self, input: QueryInput) -> impl Future<Output = Result<QueryOutput, StorageError>> + Send;
    fn scan(&self, input: ScanInput) -> impl Future<Output = Result<ScanOutput, StorageError>> + Send;

    fn batch_get_item(&self, input: BatchGetInput) -> impl Future<Output = Result<BatchGetOutput, StorageError>> + Send;
    fn batch_write_item(&self, input: BatchWriteInput) -> impl Future<Output = Result<BatchWriteOutput, StorageError>> + Send;

    fn transact_get_items(&self, input: TransactGetInput) -> impl Future<Output = Result<TransactGetOutput, StorageError>> + Send;
    fn transact_write_items(&self, input: TransactWriteInput) -> impl Future<Output = Result<TransactWriteOutput, StorageError>> + Send;
}

/// TTL and tag management.
pub trait MetadataEngine: Send + Sync {
    fn describe_ttl(&self, table_name: &str) -> impl Future<Output = Result<TtlDescription, StorageError>> + Send;
    fn update_ttl(&self, input: UpdateTtlInput) -> impl Future<Output = Result<TtlDescription, StorageError>> + Send;
    fn cleanup_expired_items(&self, table_name: &str, ttl_attribute: &str, limit: usize) -> impl Future<Output = Result<usize, StorageError>> + Send;

    fn tag_resource(&self, arn: &str, tags: Vec<Tag>) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn untag_resource(&self, arn: &str, tag_keys: Vec<String>) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn list_tags(&self, arn: &str) -> impl Future<Output = Result<Vec<Tag>, StorageError>> + Send;
}

/// DynamoDB Streams record storage and retrieval.
pub trait StreamEngine: Send + Sync {
    fn write_stream_record(&self, record: StreamRecord) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn get_stream_records(&self, input: GetRecordsInput) -> impl Future<Output = Result<GetRecordsOutput, StorageError>> + Send;
    fn describe_stream(&self, input: DescribeStreamInput) -> impl Future<Output = Result<StreamDescription, StorageError>> + Send;
    fn list_streams(&self, input: ListStreamsInput) -> impl Future<Output = Result<ListStreamsOutput, StorageError>> + Send;
}

/// Import/export job tracking.
pub trait ImportExportEngine: Send + Sync {
    fn create_import_job(&self, input: ImportJobInput) -> impl Future<Output = Result<ImportJobDescription, StorageError>> + Send;
    fn describe_import(&self, import_arn: &str) -> impl Future<Output = Result<ImportJobDescription, StorageError>> + Send;
    fn list_imports(&self, table_arn: Option<&str>) -> impl Future<Output = Result<Vec<ImportJobDescription>, StorageError>> + Send;
    fn create_export_job(&self, input: ExportJobInput) -> impl Future<Output = Result<ExportJobDescription, StorageError>> + Send;
    fn describe_export(&self, export_arn: &str) -> impl Future<Output = Result<ExportJobDescription, StorageError>> + Send;
    fn list_exports(&self, table_arn: Option<&str>) -> impl Future<Output = Result<Vec<ExportJobDescription>, StorageError>> + Send;
}

/// Credential, identity, and policy storage (storage-side trait, uses RPITIT).
///
/// Note: This is NOT the same as `CredentialStore` in the `auth` crate.
/// `CredentialEngine` is implemented by the storage backend (e.g., PostgresEngine).
/// `CredentialStore` (in `auth`) is an `#[async_trait]` object-safe trait used by
/// `BuiltinAuthProvider` via `Arc<dyn CredentialStore>`. The `bin` crate bridges
/// them with `StorageCredentialAdapter` which implements `CredentialStore` by
/// delegating to `CredentialEngine`.
///
/// Split into three focused sub-traits following Interface Segregation:
/// - `CredentialEngine` — credential CRUD
/// - `IdentityEngine` — user/role/group CRUD and tag management
/// - `SessionEngine` — session create/get/cleanup

pub trait CredentialEngine: Send + Sync {
    fn store_credential(&self, cred: StoredCredential) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn get_credential(&self, access_key_id: &str) -> impl Future<Output = Result<Option<StoredCredential>, StorageError>> + Send;
    fn deactivate_credential(&self, access_key_id: &str) -> impl Future<Output = Result<(), StorageError>> + Send;
}

pub trait IdentityEngine: Send + Sync {
    // User management
    fn create_user(&self, user: UserRecord) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn get_user(&self, user_name: &str) -> impl Future<Output = Result<Option<UserRecord>, StorageError>> + Send;
    fn delete_user(&self, user_name: &str) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn set_user_tags(&self, user_arn: &str, tags: HashMap<String, String>) -> impl Future<Output = Result<(), StorageError>> + Send;

    // Role management
    fn create_role(&self, role: RoleRecord) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn get_role(&self, role_name: &str) -> impl Future<Output = Result<Option<RoleRecord>, StorageError>> + Send;
    fn delete_role(&self, role_name: &str) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn set_role_tags(&self, role_arn: &str, tags: HashMap<String, String>) -> impl Future<Output = Result<(), StorageError>> + Send;

    // Group management
    fn create_group(&self, group_name: &str, group_arn: &str, account_id: &str) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn delete_group(&self, group_name: &str) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn add_user_to_group(&self, group_name: &str, user_name: &str) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn remove_user_from_group(&self, group_name: &str, user_name: &str) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn get_user_groups(&self, user_name: &str) -> impl Future<Output = Result<Vec<String>, StorageError>> + Send;

    // Policy management
    fn store_policy(&self, policy: StoredPolicy) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn detach_policy(&self, principal_arn: &str, policy_name: &str) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn get_policies_for_principal(&self, principal_arn: &str) -> impl Future<Output = Result<Vec<PolicyDocument>, StorageError>> + Send;
    fn set_permissions_boundary(&self, principal_arn: &str, boundary: Option<PolicyDocument>) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn get_permissions_boundary(&self, principal_arn: &str) -> impl Future<Output = Result<Option<PolicyDocument>, StorageError>> + Send;
}

/// Storage representation of an IAM policy attached to a principal.
/// Maps to the `_dynamodb_policies` table.
pub struct StoredPolicy {
    pub principal_arn: String,
    pub policy_name: String,
    pub policy_document: PolicyDocument,
    pub is_active: bool,
}

/// Storage representation of a role session (from AssumeRole).
/// Maps to the `_dynamodb_sessions` table.
pub struct SessionRecord {
    pub session_token: String,
    pub access_key_id: String,
    pub role_name: String,
    pub session_name: String,
    pub session_tags: Option<HashMap<String, String>>,
    pub session_policy: Option<PolicyDocument>,
    pub expires_at: time::OffsetDateTime,
}

pub trait SessionEngine: Send + Sync {
    fn create_session(&self, session: SessionRecord) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn get_session(&self, session_token: &str) -> impl Future<Output = Result<Option<SessionRecord>, StorageError>> + Send;
    fn revoke_session(&self, session_token: &str) -> impl Future<Output = Result<(), StorageError>> + Send;
    fn cleanup_expired_sessions(&self) -> impl Future<Output = Result<usize, StorageError>> + Send;
}

/// Convenience alias: a full storage backend implements all sub-traits.
/// The `bin` crate requires this for the concrete backend type.
pub trait StorageEngine: TableEngine + DataEngine + MetadataEngine + StreamEngine + ImportExportEngine + CredentialEngine + IdentityEngine + SessionEngine {}
impl<T> StorageEngine for T where T: TableEngine + DataEngine + MetadataEngine + StreamEngine + ImportExportEngine + CredentialEngine + IdentityEngine + SessionEngine {}
```

> **Note on object safety:** These traits use RPITIT (`impl Future` return types) which are not object-safe. Since the backend is selected once at startup, we use an enum dispatch wrapper in the `bin` crate rather than `dyn StorageEngine`. This avoids the `Box<dyn Future>` allocation per call that `async_trait` would introduce, which matters at high throughput. If runtime pluggability via dynamic dispatch is needed later, a thin `#[async_trait]` wrapper can be added around the enum.
```

## 3. Storage Input/Output Types

These types live in the `dynamodb-storage` crate and use core types. They represent the storage-layer view of each operation — stripped of HTTP concerns, expression strings already parsed.

```rust
/// Shared expression context — reused across all write input types
/// to avoid repeating expression_names + expression_values fields.
pub struct ExpressionContext {
    pub names: HashMap<String, String>,
    pub values: HashMap<String, AttributeValue>,
}

pub struct PutItemInput {
    pub table_name: String,
    pub item: Item,
    /// Pre-parsed condition expression. The storage backend evaluates this
    /// inside its transaction (after SELECT FOR UPDATE) using the core crate's
    /// `evaluate_condition` function. This prevents TOCTOU races.
    pub condition: Option<Expr>,
    pub expression_context: ExpressionContext,
    pub return_old: bool,
    /// If true, the storage backend populates `ConditionFailed.old_item`
    /// on condition failure (for ReturnValuesOnConditionCheckFailure=ALL_OLD).
    pub return_values_on_condition_failure: bool,
    /// Pre-constructed stream record (None if streams not enabled on this table).
    /// The storage backend persists this atomically with the data write.
    pub stream_record: Option<StreamRecord>,
}

pub struct PutItemOutput {
    /// The old item (if return_old was true and item existed).
    pub old_item: Option<Item>,
}

pub struct GetItemInput {
    pub table_name: String,
    pub key: Item,  // partition key + optional sort key
    pub consistent_read: bool,
}

pub struct GetItemOutput {
    pub item: Option<Item>,
}

pub struct QueryInput {
    pub table_name: String,
    pub index_name: Option<String>,
    pub key_condition: KeyCondition,
    pub scan_forward: bool,
    pub limit: Option<usize>,
    pub exclusive_start_key: Option<Item>,
    pub consistent_read: bool,
}

/// Parsed key condition — the storage backend translates this to native queries.
pub struct KeyCondition {
    pub partition_key_name: String,
    pub partition_key_value: AttributeValue,
    pub sort_condition: Option<SortKeyCondition>,
}

pub enum SortKeyCondition {
    Eq(AttributeValue),
    Lt(AttributeValue),
    Le(AttributeValue),
    Gt(AttributeValue),
    Ge(AttributeValue),
    Between(AttributeValue, AttributeValue),
    BeginsWith(String),
}

pub struct QueryOutput {
    pub items: Vec<Item>,
    pub last_evaluated_key: Option<Item>,
    pub scanned_count: usize,
}

pub struct ScanInput {
    pub table_name: String,
    pub index_name: Option<String>,
    pub limit: Option<usize>,
    pub exclusive_start_key: Option<Item>,
    pub segment: Option<usize>,
    pub total_segments: Option<usize>,
    pub consistent_read: bool,
}

pub struct ScanOutput {
    pub items: Vec<Item>,
    pub last_evaluated_key: Option<Item>,
    pub scanned_count: usize,
}

pub struct TransactWriteInput {
    pub items: Vec<TransactWriteItem>,
    pub client_request_token: Option<String>,
}

pub enum TransactWriteItem {
    Put(TransactPut),
    Delete(TransactDelete),
    Update(TransactUpdate),
    ConditionCheck(TransactConditionCheck),
}

pub struct TransactPut {
    pub table_name: String,
    pub item: Item,
    pub condition: Option<Expr>,
    pub expression_context: ExpressionContext,
    pub return_values_on_condition_failure: bool,
    pub stream_record: Option<StreamRecord>,
}

pub struct TransactDelete {
    pub table_name: String,
    pub key: Item,
    pub condition: Option<Expr>,
    pub expression_context: ExpressionContext,
    pub return_values_on_condition_failure: bool,
    pub stream_record: Option<StreamRecord>,
}

pub struct TransactUpdate {
    pub table_name: String,
    pub key: Item,
    pub updates: Vec<UpdateAction>,
    pub condition: Option<Expr>,
    pub expression_context: ExpressionContext,
    pub return_values_on_condition_failure: bool,
    /// Stream view type + metadata. The storage backend constructs the
    /// full stream record (with new_image) after applying the update.
    pub stream_capture: Option<StreamCapture>,
}

pub struct TransactConditionCheck {
    pub table_name: String,
    pub key: Item,
    pub condition: Expr,
    pub expression_context: ExpressionContext,
    pub return_values_on_condition_failure: bool,
}

pub struct DeleteItemInput {
    pub table_name: String,
    pub key: Item,
    pub condition: Option<Expr>,
    pub expression_context: ExpressionContext,
    pub return_old: bool,
    pub return_values_on_condition_failure: bool,
    pub stream_record: Option<StreamRecord>,
}

pub struct DeleteItemOutput {
    pub old_item: Option<Item>,
}

pub struct UpdateItemInput {
    pub table_name: String,
    pub key: Item,
    /// Pre-parsed update actions. The storage backend calls
    /// `core::expression::apply_update()` inside its transaction
    /// after SELECT FOR UPDATE to produce the modified item.
    pub updates: Vec<UpdateAction>,
    pub condition: Option<Expr>,
    pub expression_context: ExpressionContext,
    pub return_values: ReturnValues,
    pub return_values_on_condition_failure: bool,
    /// Stream capture metadata. Unlike PutItem/DeleteItem, UpdateItem cannot
    /// pre-construct the full StreamRecord because the new_image is not known
    /// until after apply_update runs inside the transaction. Instead, the engine
    /// passes stream metadata and the storage backend constructs the full record.
    pub stream_capture: Option<StreamCapture>,
}

/// Metadata for stream record construction inside the storage transaction.
/// Used by UpdateItem and TransactUpdate where the new_image is not known
/// until after the update expression is applied.
pub struct StreamCapture {
    pub stream_arn: String,
    pub stream_view_type: StreamViewType,
    pub shard_id: String,
    pub sequence_number: String,
    pub event_name: StreamEventName,
    pub keys: BTreeMap<String, AttributeValue>,
}

pub enum ReturnValues { None, AllOld, UpdatedOld, AllNew, UpdatedNew }

pub struct UpdateItemOutput {
    pub attributes: Option<Item>,
}

pub struct BatchGetInput {
    pub tables: HashMap<String, BatchGetTableRequest>,
}

pub struct BatchGetTableRequest {
    pub keys: Vec<Item>,
    pub consistent_read: bool,
}

pub struct BatchGetOutput {
    pub responses: HashMap<String, Vec<Item>>,
    pub unprocessed_keys: HashMap<String, BatchGetTableRequest>,
}

pub struct BatchWriteInput {
    pub tables: HashMap<String, Vec<WriteRequest>>,
}

/// BatchWriteItem requests are simpler than individual PutItem/DeleteItem:
/// no condition expressions, no ReturnValues, no ReturnValuesOnConditionCheckFailure.
/// The table name comes from the HashMap key in BatchWriteInput.tables.
pub enum WriteRequest {
    Put { item: Item, stream_record: Option<StreamRecord> },
    Delete { key: Item, stream_record: Option<StreamRecord> },
}

pub struct BatchWriteOutput {
    pub unprocessed_items: HashMap<String, Vec<WriteRequest>>,
}

pub struct TransactGetInput {
    pub items: Vec<TransactGetItem>,
}

pub struct TransactGetItem {
    pub table_name: String,
    pub key: Item,
}

pub struct TransactGetOutput {
    pub responses: Vec<Option<Item>>,
}

pub struct TransactWriteOutput {
    pub item_collection_metrics: Option<HashMap<String, Vec<ItemCollectionMetrics>>>,
}
```

## 4. StorageError

```rust
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("Item not found")]
    NotFound,
    #[error("Table not found: {0}")]
    TableNotFound(String),
    #[error("Table already exists: {0}")]
    TableAlreadyExists(String),
    #[error("Table is not in ACTIVE state: {0}")]
    TableNotActive(String),
    #[error("Condition check failed")]
    ConditionFailed { old_item: Option<Item> },
    #[error("Transaction conflict")]
    TransactionConflict,
    #[error("Transaction cancelled: {0}")]
    TransactionCancelled(Vec<Option<String>>),  // per-item cancellation reasons
    #[error("Connection error: {0}")]
    Connection(String),
    #[error("Internal error: {0}")]
    Internal(String),
}
```

The `server` crate maps `StorageError` variants to `DynamoDbError` variants for the HTTP response:

| `StorageError` | `DynamoDbError` | HTTP Status |
|---|---|---|
| `NotFound` | (no mapping — item not found is a valid empty response, not an error) | 200 |
| `TableNotFound(name)` | `ResourceNotFoundException` | 400 |
| `TableAlreadyExists(name)` | `ResourceInUseException` | 400 |
| `TableNotActive(name)` | `ResourceInUseException` ("Table is being created/deleted/updated") | 400 |
| `ConditionFailed { old_item }` | `ConditionalCheckFailedException` (with old item in response if `return_values_on_condition_failure` was set) | 400 |
| `TransactionConflict` | `TransactionConflictException` | 400 |
| `TransactionCancelled(reasons)` | `TransactionCanceledException` (with per-item `CancellationReasons`) | 400 |
| `Connection(msg)` | `ServiceUnavailable` | 503 |
| `Internal(msg)` | `InternalServerError` | 500 |

## 5. PostgreSQL Backend

### 5.1 Schema Design

**Metadata tables** (created by migrations):

```sql
-- Table metadata
CREATE TABLE tables (
    table_name TEXT PRIMARY KEY,
    key_schema JSONB NOT NULL,
    attribute_definitions JSONB NOT NULL,
    billing_mode TEXT NOT NULL DEFAULT 'PAY_PER_REQUEST',
    provisioned_throughput JSONB,
    stream_specification JSONB,
    table_status TEXT NOT NULL DEFAULT 'ACTIVE',
    creation_date_time TIMESTAMPTZ NOT NULL DEFAULT now(),
    table_size_bytes BIGINT DEFAULT 0,
    item_count BIGINT DEFAULT 0,
    table_arn TEXT NOT NULL,
    table_id TEXT NOT NULL,
    ttl_attribute TEXT
);

-- Index metadata (FK on table_id, not table_name)
CREATE TABLE indexes (
    table_id TEXT NOT NULL REFERENCES tables(table_id) ON DELETE CASCADE,
    index_name TEXT NOT NULL,
    index_type TEXT NOT NULL,  -- 'GSI' or 'LSI'
    key_schema JSONB NOT NULL,
    projection JSONB NOT NULL,
    index_status TEXT NOT NULL DEFAULT 'ACTIVE',
    provisioned_throughput JSONB,
    PRIMARY KEY (table_id, index_name)
);

-- Resource tags
CREATE TABLE tags (
    resource_arn TEXT NOT NULL,
    tag_key TEXT NOT NULL,
    tag_value TEXT NOT NULL,
    PRIMARY KEY (resource_arn, tag_key)
);

-- Credentials (for built-in auth)
CREATE TABLE _dynamodb_credentials (
    access_key_id TEXT PRIMARY KEY,
    secret_key_enc BYTEA NOT NULL,
    principal_type TEXT NOT NULL,  -- 'user' or 'session'
    principal_name TEXT NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- IAM users
CREATE TABLE _dynamodb_users (
    user_name TEXT PRIMARY KEY,
    user_arn TEXT NOT NULL UNIQUE,
    account_id TEXT NOT NULL,
    permissions_boundary_arn TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- IAM roles
CREATE TABLE _dynamodb_roles (
    role_name TEXT PRIMARY KEY,
    role_arn TEXT NOT NULL UNIQUE,
    account_id TEXT NOT NULL,
    trust_policy JSONB NOT NULL,
    permissions_boundary_arn TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- IAM groups
CREATE TABLE _dynamodb_groups (
    group_name TEXT PRIMARY KEY,
    group_arn TEXT NOT NULL UNIQUE,
    account_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Group membership (user → groups)
CREATE TABLE _dynamodb_group_members (
    group_name TEXT NOT NULL REFERENCES _dynamodb_groups(group_name),
    user_name TEXT NOT NULL REFERENCES _dynamodb_users(user_name),
    PRIMARY KEY (group_name, user_name)
);

-- Principal tags (for users and roles)
CREATE TABLE _dynamodb_principal_tags (
    principal_arn TEXT NOT NULL,
    tag_key TEXT NOT NULL,
    tag_value TEXT NOT NULL,
    PRIMARY KEY (principal_arn, tag_key)
);

-- IAM policies (attached to users, groups, or roles)
CREATE TABLE _dynamodb_policies (
    policy_id TEXT PRIMARY KEY,
    principal_arn TEXT NOT NULL,   -- user ARN, group ARN, or role ARN
    policy_name TEXT NOT NULL,
    policy_document JSONB NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT true,
    UNIQUE (principal_arn, policy_name)
);

-- Role sessions (temporary credentials from AssumeRole)
CREATE TABLE _dynamodb_sessions (
    session_token TEXT PRIMARY KEY,
    access_key_id TEXT NOT NULL UNIQUE,
    role_name TEXT NOT NULL REFERENCES _dynamodb_roles(role_name),
    session_name TEXT NOT NULL,
    session_tags JSONB,           -- tags passed at assumption time
    session_policy JSONB,         -- inline session policy (if any)
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ON _dynamodb_sessions(expires_at);

-- Stream records
CREATE TABLE _dynamodb_stream_records (
    id BIGSERIAL PRIMARY KEY,
    stream_arn TEXT NOT NULL,
    shard_id TEXT NOT NULL,
    sequence_number TEXT NOT NULL,
    event_name TEXT NOT NULL,
    table_name TEXT NOT NULL,
    keys JSONB,
    old_image JSONB,
    new_image JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ON _dynamodb_stream_records(stream_arn, shard_id, sequence_number);

-- Import/export job tracking
CREATE TABLE _dynamodb_import_jobs (
    import_arn TEXT PRIMARY KEY,
    table_name TEXT NOT NULL,
    input_format TEXT NOT NULL,
    import_status TEXT NOT NULL DEFAULT 'IN_PROGRESS',
    import_data JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE _dynamodb_export_jobs (
    export_arn TEXT PRIMARY KEY,
    table_name TEXT NOT NULL,
    export_status TEXT NOT NULL DEFAULT 'IN_PROGRESS',
    export_data JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Idempotency tokens for TransactWriteItems
CREATE TABLE _dynamodb_idempotency_tokens (
    client_request_token TEXT PRIMARY KEY,
    response JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ON _dynamodb_idempotency_tokens(created_at);
```

**Per-DynamoDB-table** (created dynamically by `create_table`):

```sql
-- For a table "Users" with partition key "user_id" (S) and sort key "timestamp" (N):
CREATE TABLE _ddb_Users (
    pk TEXT NOT NULL,              -- partition key value (always TEXT — S, N, and B all stored as text)
    sk_s TEXT,                     -- sort key value when type is S
    sk_n NUMERIC,                  -- sort key value when type is N (correct numeric ordering)
    sk_b BYTEA,                    -- sort key value when type is B
    item_data JSONB NOT NULL,      -- full item including key attributes
    PRIMARY KEY (pk, sk_n)         -- PK uses the column matching the sort key type
);

-- For a table with no sort key:
CREATE TABLE _ddb_SimpleTable (
    pk TEXT NOT NULL PRIMARY KEY,
    item_data JSONB NOT NULL
);

-- For a GSI "EmailIndex" on "Users" (GSI pk = email (S), GSI sk = none):
-- GSI keys are NOT unique — multiple base table items can project to the same GSI key.
-- The base table primary key columns are included to ensure uniqueness.
CREATE TABLE _ddb_Users__gsi__EmailIndex (
    pk TEXT NOT NULL,              -- GSI partition key
    sk_s TEXT,                     -- GSI sort key (type depends on GSI key schema)
    sk_n NUMERIC,
    sk_b BYTEA,
    base_pk TEXT NOT NULL,         -- base table partition key (for uniqueness + pagination)
    base_sk_n NUMERIC,             -- base table sort key (type matches base table; NULL if no sort key)
    item_data JSONB NOT NULL,
    PRIMARY KEY (pk, base_pk, base_sk_n)  -- GSI pk + base table pk for uniqueness
);
-- Index for sort key ordering within a GSI partition:
CREATE INDEX ON _ddb_Users__gsi__EmailIndex (pk, sk_s, base_pk, base_sk_n);
```

**Design notes:**
- All DynamoDB table names are prefixed with `_ddb_` to avoid collisions with metadata tables.
- Partition key values are always stored as `TEXT` — string keys store directly, number keys store their string representation, binary keys store base64. Partition keys only need equality comparison, so text storage is correct. **Important:** Binary partition keys must use canonical base64 encoding (standard alphabet with padding, via `base64::engine::general_purpose::STANDARD`) to ensure equality comparison is reliable. A validation step on ingest must normalize the encoding.
- Sort key values use typed columns (`sk_s TEXT`, `sk_n NUMERIC`, `sk_b BYTEA`) to ensure correct ordering. Only one `sk_*` column is populated per table, determined by the sort key's `AttributeDefinition` type. The `CREATE TABLE` DDL and `PRIMARY KEY` constraint are generated dynamically based on the key schema.
  - `NUMERIC` ensures `2 < 10 < 100` (not lexicographic `"10" < "2"`).
  - `BYTEA` ensures correct binary comparison order.
  - `TEXT` ensures correct UTF-8 string ordering.
- `item_data` JSONB contains the complete item including key attributes, matching the DynamoDB model where key attributes are part of the item.
- **GSI tables include base table primary key columns** (`base_pk`, `base_sk_*`) as actual SQL columns (not just inside `item_data` JSONB). This is required because: (1) GSI keys are not unique — two base table items can project to the same GSI key, so the base table PK is needed for uniqueness; (2) pagination requires a tiebreaker when GSI keys collide; (3) the base table PK is needed to look up the full item for projections.
- GSI tables are maintained synchronously within the same transaction as the base table write (see §6 for GSI consistency discussion).

### 5.2 Connection Pooling

```rust
use sqlx::postgres::PgPoolOptions;

pub struct PostgresEngine {
    /// Primary connection pool — used for all writes and consistent reads.
    pool: PgPool,
    /// Optional read replica pool — used for eventually consistent reads
    /// (ConsistentRead=false). When None, all reads use the primary pool.
    read_pool: Option<PgPool>,
}

impl PostgresEngine {
    pub async fn new(config: &PostgresConfig) -> Result<Self, StorageError> {
        let pool = PgPoolOptions::new()
            .max_connections(config.pool_size)
            .connect(&config.connection_string)
            .await
            .map_err(|e| StorageError::Connection(e.to_string()))?;

        let read_pool = match &config.read_replica_url {
            Some(url) => {
                let rp = PgPoolOptions::new()
                    .max_connections(config.read_replica_pool_size.unwrap_or(config.pool_size))
                    .connect(url)
                    .await
                    .map_err(|e| StorageError::Connection(format!("read replica: {e}")))?;
                Some(rp)
            }
            None => None,
        };

        Ok(Self { pool, read_pool })
    }

    /// Returns the appropriate connection pool for a read operation.
    /// Uses the read replica for eventually consistent reads (when available),
    /// falls back to the primary pool otherwise.
    fn read_pool(&self, consistent_read: bool) -> &PgPool {
        if consistent_read {
            &self.pool
        } else {
            self.read_pool.as_ref().unwrap_or(&self.pool)
        }
    }
}
```

### 5.3 Read Consistency Model

DynamoDB supports two read consistency modes: strongly consistent and eventually consistent (the default). The PostgreSQL backend models this via an optional read replica.

**Single-node mode (no read replica configured):** All reads are strongly consistent regardless of the `consistent_read` flag. This is strictly stronger than the DynamoDB spec and compatible with all applications. The `consistent_read` parameter is accepted and correctly reflected in capacity calculations (eventually consistent reads consume 0.5 RCU vs 1.0 RCU for strongly consistent).

**Read replica mode (`read_replica_url` configured):** Eventually consistent reads (`consistent_read=false`) are routed to a PostgreSQL streaming replica that is naturally a few milliseconds behind the primary. Strongly consistent reads (`consistent_read=true`) always read from the primary. This mirrors how DynamoDB achieves eventual consistency — via storage node replicas — and surfaces the exact class of bugs that applications may encounter in production DynamoDB (e.g., read-after-write without `ConsistentRead=true` returning stale data).

**Which operations are affected:**
- `GetItem`: uses `consistent_read` field (default `false` in DynamoDB)
- `Query`: uses `consistent_read` field (default `false` in DynamoDB)
- `Scan`: uses `consistent_read` field (default `false` in DynamoDB)
- `BatchGetItem`: uses per-table `consistent_read` field
- `TransactGetItems`: always strongly consistent (DynamoDB spec — serializable isolation)
- All write operations: always use the primary pool

The `read_pool()` helper method on `PostgresEngine` encapsulates this routing. All read implementations call `self.read_pool(input.consistent_read)` instead of `&self.pool` directly.

### 5.4 Query Translation

The storage backend translates `KeyCondition` to SQL:

```rust
// KeyCondition { pk_name: "user_id", pk_value: "alice", sort: Some(BeginsWith("2024")) }
// →
// SELECT item_data FROM _ddb_Users WHERE pk = $1 AND sk_s >= $2 AND sk_s < $3
// params: ["alice", "2024", "2025"]  -- $3 is prefix with last char incremented
```

For sort key conditions:
| SortKeyCondition | SQL |
|-----------------|-----|
| `Eq(v)` | `sk_x = $n` |
| `Lt(v)` | `sk_x < $n` |
| `Le(v)` | `sk_x <= $n` |
| `Gt(v)` | `sk_x > $n` |
| `Ge(v)` | `sk_x >= $n` |
| `Between(a, b)` | `sk_x BETWEEN $n AND $m` |
| `BeginsWith(s)` | `sk_x >= $n AND sk_x < $m` (where `$m` = prefix upper bound, see algorithm below) |

> **Note on `sk_x`:** The actual column name (`sk_s`, `sk_n`, `sk_b`) is determined by the sort key's `AttributeDefinition` type, looked up from table metadata at query time. `BeginsWith` only applies to `S` and `B` type sort keys.

> **Note on `BeginsWith`:** Using a range scan (`>= prefix AND < prefix_next`) instead of SQL `LIKE` avoids two problems: (1) `%` and `_` characters in the prefix would be interpreted as LIKE wildcards, causing incorrect matches; (2) range scans are more B-tree index friendly than LIKE patterns.

> **`BeginsWith` upper bound algorithm:** The upper bound is computed by stripping trailing `0xFF` bytes, then incrementing the last non-`0xFF` byte. If the prefix is entirely `0xFF` bytes, there is no upper bound (scan to end of partition). For string sort keys, operate on raw UTF-8 bytes, not characters.

```rust
fn prefix_upper_bound(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut upper = prefix.to_vec();
    while upper.last() == Some(&0xFF) {
        upper.pop();
    }
    if upper.is_empty() {
        return None; // all 0xFF — no upper bound, scan to end
    }
    *upper.last_mut().unwrap() += 1;
    Some(upper)
}
// When None is returned, the SQL omits the upper bound:
//   sk_x >= $n  (no AND sk_x < $m)
```

### 5.5 Transaction Support

TransactWriteItems maps to a PostgreSQL transaction:

```rust
async fn transact_write_items(&self, input: TransactWriteInput) -> Result<...> {
    let mut tx = self.pool.begin().await?;
    for item in &input.items {
        match item {
            TransactWriteItem::Put { .. } => { /* INSERT/UPSERT within tx */ }
            TransactWriteItem::Delete { .. } => { /* DELETE within tx */ }
            TransactWriteItem::Update { .. } => { /* UPDATE within tx */ }
            TransactWriteItem::ConditionCheck { .. } => { /* SELECT + evaluate */ }
        }
    }
    tx.commit().await?;
    Ok(...)
}
```

### 5.6 Migrations

Migrations are embedded in the binary at compile time via `include_str!` and applied in order by the `catalog::run_migrations` helper. Each migration is tracked in the `schema_history` table.

Migration files are numbered sequentially:
```
migrations/
└── 001_initial_schema.sql
```

## 6. GSI Consistency Model

**Decision:** GSI updates are synchronous — they happen in the same PostgreSQL transaction as the base table write.

**Rationale:**
- Simplicity: no background backfill worker, no eventual consistency window, no stale reads from GSIs
- Correctness: a successful PutItem guarantees the GSI is immediately queryable
- PostgreSQL transactions are cheap for this use case (2-3 additional INSERTs per write)

**Trade-off:** This differs from real DynamoDB where GSI updates are eventually consistent. A GSI write failure will roll back the base table write. This is acceptable for a compatibility layer — applications that depend on GSI eventual consistency semantics are rare, and the synchronous model is strictly stronger.

### 6.1 Table Status Enforcement

All data plane operations (PutItem, GetItem, Query, etc.) must check `table_status` before proceeding. If the table is not `ACTIVE`, return `StorageError::TableNotActive` (mapped to `ResourceInUseException`). Control plane operations that modify the table (`UpdateTable`, `DeleteTable`) must:

1. Atomically set `table_status` to `UPDATING` or `DELETING` (using `UPDATE ... WHERE table_status = 'ACTIVE'` — if zero rows affected, the table is already being modified, return `ResourceInUseException`)
2. Perform the operation
3. Set `table_status` back to `ACTIVE` (or remove the row for `DeleteTable`)

This prevents concurrent DDL operations on the same table.

### 6.1.1 Async Control Plane Transitions (Phase 1c)

Real DynamoDB control plane operations are not instantaneous — `CreateTable` returns `CREATING` status and the table transitions to `ACTIVE` asynchronously. extenddb emulates this behavior.

**Implementation:**

- A `status_transition_at TIMESTAMPTZ` column on the `tables` table records when a pending transition should fire. When `NULL`, no transition is pending.
- `CreateTable` inserts with `table_status = 'CREATING'` and sets `status_transition_at` to `NOW() + control_plane_delay_seconds`. The delay is read from the settings table via a subquery in the same INSERT (no extra round-trip).
- `DeleteTable` sets `table_status = 'DELETING'` with a scheduled transition time. The row, its indexes, and tags are removed when the transition fires.
- A background poller processes pending transitions. `CREATING → ACTIVE` is a single UPDATE; `DELETING → removed` uses `DELETE ... FOR UPDATE SKIP LOCKED ... RETURNING` for concurrent safety.
- On startup, `process_control_plane_transitions()` recovers any in-flight operations from a previous server instance.
- A partial index (`idx_tables_pending_transition ON tables (status_transition_at) WHERE status_transition_at IS NOT NULL`) keeps the poller query efficient regardless of table count.

**Design decisions and future direction (from Phase 1c human review):**

- The single-column approach works because each table has exactly one pending status transition at a time. Index-level transitions (e.g., GSI backfill) will need a separate `status_transition_at` on the `indexes` table when GSI operations are implemented.
- The poller interval will be increased to 10 seconds at idle, with control plane operations poking the poller to wake up immediately and backoff appropriately (Phase 2).
- The default delay will be randomized to `[5, 20]` seconds for more realistic DynamoDB emulation (Phase 2).
- Startup recovery will reset stuck `CREATING` tables to `NOW() + random[5, 20]` instead of instant activation (Phase 2).
- `control_plane_delay_seconds` is a runtime setting (0–300 range), managed via `extenddb settings set`. It is not a `.toml` config key.

**Crash recovery and in-flight operation tracking:**

The `status_transition_at` column on the `tables` table serves as the in-flight operation tracker. When the extenddb server shuts down (cleanly or via crash) while tables have pending transitions, the state is durable in PostgreSQL. On the next startup, `process_control_plane_transitions()` scans for rows where `status_transition_at IS NOT NULL AND status_transition_at <= NOW()` and completes them immediately. Rows where `status_transition_at` is in the future are left for the background poller.

This column-on-tables approach is sufficient while control plane operations map 1:1 to table status changes (`CREATING → ACTIVE`, `DELETING → removed`). A separate `control_plane_operations` table becomes necessary when:
- Operations span multiple catalog entities (e.g., GSI backfill touches both `indexes` and data tables)
- Operations have intermediate states beyond a single status flip (e.g., multi-step UpdateTable)
- Audit or observability requires a history of completed operations, not just pending ones

Until those requirements arise, the single-column approach avoids the complexity of a separate job queue while providing full crash recovery.

### 6.2 GSI Backfill on CreateIndex

When `UpdateTable` adds a new GSI to a table with existing data:

1. Set the new index status to `CREATING` in `indexes`
2. Spawn a background task that scans the base table in batches (configurable batch size, default 1000)
3. For each batch, INSERT the projected attributes into the new GSI table
4. On completion, set index status to `ACTIVE`
5. During backfill, writes to the base table also write to the new GSI table (the write path checks index status and includes `CREATING` indexes)
6. Queries against a `CREATING` index return `ResourceNotFoundException` (matching DynamoDB behavior)

## 7. Pagination Token Encoding

`ExclusiveStartKey` and `LastEvaluatedKey` use the same format: a map of key attribute names to `AttributeValue`s, serialized as standard DynamoDB JSON.

```rust
/// LastEvaluatedKey is the primary key of the last item evaluated.
/// For a base table: { "pk_name": {"S": "val"}, "sk_name": {"N": "42"} }
/// For a GSI: { "gsi_pk": {"S": "val"}, "gsi_sk": {"S": "val"}, "table_pk": {"S": "val"}, "table_sk": {"N": "42"} }
pub type PaginationKey = BTreeMap<String, AttributeValue>;
```

The storage backend translates this to a SQL `WHERE` clause:

**Base table pagination (forward scan):**
```sql
WHERE (pk = $last_pk AND sk_n > $last_sk)
   OR pk > $last_pk
```

**Base table pagination (reverse scan):**
```sql
WHERE (pk = $last_pk AND sk_n < $last_sk)
   OR pk < $last_pk
```

**GSI pagination (forward scan):**
GSI keys are not unique, so the base table primary key is used as a tiebreaker:
```sql
WHERE (pk = $gsi_pk AND sk_s > $gsi_sk)
   OR (pk = $gsi_pk AND sk_s = $gsi_sk AND base_pk > $base_pk)
   OR (pk = $gsi_pk AND sk_s = $gsi_sk AND base_pk = $base_pk AND base_sk_n > $base_sk)
   OR pk > $gsi_pk
```

For GSI queries, the pagination key includes both the GSI key attributes and the base table primary key (needed to uniquely identify the position, since GSI keys are not unique). This is why the GSI PostgreSQL table includes `base_pk` and `base_sk_*` as actual columns.

## 8. Parallel Scan Segment Assignment

`Segment` and `TotalSegments` map to PostgreSQL via hash-based partitioning of the primary key:

```sql
-- Scan segment 2 of 4 total segments:
SELECT item_data FROM _ddb_Users
WHERE (hashtext(pk)::bigint & x'7FFFFFFF'::bigint) % 4 = 2
ORDER BY pk, sk_s
LIMIT $limit;
```

`hashtext()` is a built-in PostgreSQL function that produces a deterministic int32 hash. We cast to `bigint` and mask with `0x7FFFFFFF` to ensure a non-negative result (avoiding the `abs(INT_MIN)` overflow edge case where `abs(-2147483648)` returns a negative value in PostgreSQL). Using modulo arithmetic assigns each partition key to exactly one segment, ensuring:
- Every item appears in exactly one segment (no duplicates, no gaps)
- Segments can be scanned in parallel by independent workers
- The assignment is deterministic (same item always in same segment)

> **Portability note:** `hashtext()` is PostgreSQL-specific. Segment assignment is not guaranteed to be consistent across different storage backends. If cross-backend consistency is needed in the future, define a hash function in the `core` crate (e.g., CRC32 of the partition key bytes) that all backends use, and pass the pre-computed segment filter to the storage backend.

## 9. Idempotency Token Storage

`TransactWriteItems` supports `ClientRequestToken` for idempotency. Tokens are stored in a dedicated table:

```sql
CREATE TABLE _dynamodb_idempotency_tokens (
    client_request_token TEXT PRIMARY KEY,
    response JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ON _dynamodb_idempotency_tokens(created_at);
```

**Flow:**
1. Before executing a transaction, check if the token exists
2. If found: return the stored response (idempotent replay)
3. If not found: execute the transaction, store the token + response atomically in the same PostgreSQL transaction
4. Background cleanup: delete tokens older than 10 minutes (matching DynamoDB's idempotency window)

If a request arrives with the same token but different parameters, return `IdempotentParameterMismatchException`.

## 10. Adding a New Backend

To add a new storage backend (e.g., SQLite):

1. Create a new crate: `crates/storage-sqlite/`
2. Add dependencies: `dynamodb-storage`, `dynamodb-core`, `sqlx` (with sqlite feature)
3. Implement the required sub-traits (`TableEngine`, `DataEngine`, etc.) for `SqliteEngine`
4. Register the backend in `crates/bin/src/main.rs` config loading (add to the enum dispatch wrapper)
5. No changes to any existing crate

---

## License

Copyright 2026 ExtendDB contributors. Licensed under the Apache License, Version 2.0.
See [LICENSE](../../LICENSE) for the full text.

This software is provided "as is" without warranty of any kind. ExtendDB is not
affiliated with, endorsed by, or sponsored by Amazon Web Services. "DynamoDB" is a trademark
of Amazon.com, Inc.
