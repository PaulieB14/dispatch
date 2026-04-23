use sqlx::Row;

use crate::{db::Pool, tap::ValidatedReceipt};

/// Persist a validated TAP receipt to PostgreSQL.
///
/// Returns the auto-assigned row `id`.
pub async fn insert(pool: &Pool, chain_id: u64, validated: &ValidatedReceipt) -> anyhow::Result<i64> {
    let row = sqlx::query(
        r#"
        INSERT INTO tap_receipts
            (signer_address, payer_address, chain_id, timestamp_ns, nonce, value, signature, metadata, method)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id
        "#,
    )
    .bind(format!("{:?}", validated.signer))   // gateway signer (authorized_senders)
    .bind(format!("{:?}", validated.payer))    // consumer — whose escrow is charged
    .bind(chain_id as i64)
    .bind(validated.receipt.timestamp_ns as i64)
    .bind(validated.receipt.nonce as i64)
    .bind(validated.receipt.value.to_string()) // u128 → decimal string
    .bind(&validated.signature)
    .bind(validated.receipt.metadata.as_ref()) // &[u8]
    .bind(&validated.method)
    .fetch_one(pool)
    .await?;

    Ok(row.get("id"))
}

// ---------------------------------------------------------------------------
// Receipt feed API helpers
// ---------------------------------------------------------------------------

/// A lightweight receipt row for the dashboard feed and consumer history.
pub struct ReceiptRow {
    pub id: i64,
    pub payer_address: String,
    pub chain_id: i64,
    pub timestamp_ns: i64,
    pub value: String,
    pub method: Option<String>,
}

/// Fetch the most recent receipts across all consumers, newest first.
pub async fn recent(pool: &Pool, limit: i64) -> anyhow::Result<Vec<ReceiptRow>> {
    let rows = sqlx::query(
        r#"
        SELECT id, payer_address, chain_id, timestamp_ns, value, method
        FROM   tap_receipts
        ORDER  BY timestamp_ns DESC
        LIMIT  $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ReceiptRow {
            id: r.get("id"),
            payer_address: r.get("payer_address"),
            chain_id: r.get("chain_id"),
            timestamp_ns: r.get("timestamp_ns"),
            value: r.get("value"),
            method: r.get("method"),
        })
        .collect())
}

/// Fetch the most recent receipts for a specific consumer, newest first.
pub async fn by_payer_recent(pool: &Pool, payer_hex: &str, limit: i64) -> anyhow::Result<Vec<ReceiptRow>> {
    let rows = sqlx::query(
        r#"
        SELECT id, payer_address, chain_id, timestamp_ns, value, method
        FROM   tap_receipts
        WHERE  payer_address = $1
        ORDER  BY timestamp_ns DESC
        LIMIT  $2
        "#,
    )
    .bind(payer_hex)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ReceiptRow {
            id: r.get("id"),
            payer_address: r.get("payer_address"),
            chain_id: r.get("chain_id"),
            timestamp_ns: r.get("timestamp_ns"),
            value: r.get("value"),
            method: r.get("method"),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Aggregator helpers
// ---------------------------------------------------------------------------

/// A raw receipt row fetched for RAV aggregation.
pub struct RawReceipt {
    pub id: i64,
    pub payer_address: String,
    pub timestamp_ns: i64,
    pub nonce: i64,
    pub value: String,         // decimal u128
    pub signature: String,
    pub metadata: Vec<u8>,
}

/// Fetch all receipts for `payer_hex` (consumer address, e.g. "0xabc…").
/// Returns them oldest-first for deterministic ordering.
pub async fn fetch_by_payer(pool: &Pool, payer_hex: &str) -> anyhow::Result<Vec<RawReceipt>> {
    let rows = sqlx::query(
        r#"
        SELECT id, payer_address, timestamp_ns, nonce, value, signature, metadata
        FROM   tap_receipts
        WHERE  payer_address = $1
        ORDER  BY timestamp_ns ASC
        "#,
    )
    .bind(payer_hex)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RawReceipt {
            id: r.get("id"),
            payer_address: r.get("payer_address"),
            timestamp_ns: r.get("timestamp_ns"),
            nonce: r.get("nonce"),
            value: r.get("value"),
            signature: r.get("signature"),
            metadata: r.get("metadata"),
        })
        .collect())
}

/// Return the distinct consumer (payer) addresses present in tap_receipts.
pub async fn distinct_payers(pool: &Pool) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query("SELECT DISTINCT payer_address FROM tap_receipts")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|r| r.get("payer_address")).collect())
}

// ---------------------------------------------------------------------------
// RAV upsert
// ---------------------------------------------------------------------------

pub struct RavRow<'a> {
    pub collection_id: &'a str,
    pub payer_address: &'a str,
    pub service_provider: &'a str,
    pub data_service: &'a str,
    pub timestamp_ns: i64,
    pub value_aggregate: &'a str,
    pub signature: &'a str,
    pub last_updated: i64,
}

// ---------------------------------------------------------------------------
// Collector helpers
// ---------------------------------------------------------------------------

/// A RAV row ready for on-chain submission.
pub struct RedeemableRav {
    pub collection_id: String,
    pub payer_address: String,
    pub service_provider: String,
    pub data_service: String,
    pub timestamp_ns: i64,
    pub value_aggregate: String,
    pub signature: String,
}

/// Fetch all RAVs that have not yet been submitted on-chain.
pub async fn fetch_unredeemed_ravs(pool: &Pool) -> anyhow::Result<Vec<RedeemableRav>> {
    let rows = sqlx::query(
        r#"
        SELECT collection_id, payer_address, service_provider, data_service,
               timestamp_ns, value_aggregate, signature
        FROM   tap_ravs
        WHERE  redeemed = false
        ORDER  BY last_updated ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RedeemableRav {
            collection_id: r.get("collection_id"),
            payer_address: r.get("payer_address"),
            service_provider: r.get("service_provider"),
            data_service: r.get("data_service"),
            timestamp_ns: r.get("timestamp_ns"),
            value_aggregate: r.get("value_aggregate"),
            signature: r.get("signature"),
        })
        .collect())
}

/// Mark a RAV as redeemed after successful on-chain collection.
pub async fn mark_rav_redeemed(pool: &Pool, collection_id: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE tap_ravs SET redeemed = true WHERE collection_id = $1")
        .bind(collection_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Insert or update the RAV for a given collection_id.
/// `value_aggregate` and `timestamp_ns` are always replaced with the latest values.
pub async fn upsert_rav(pool: &Pool, rav: RavRow<'_>) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO tap_ravs
            (collection_id, payer_address, service_provider, data_service,
             timestamp_ns, value_aggregate, signature, last_updated)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (collection_id) DO UPDATE SET
            timestamp_ns    = EXCLUDED.timestamp_ns,
            value_aggregate = EXCLUDED.value_aggregate,
            signature       = EXCLUDED.signature,
            last_updated    = EXCLUDED.last_updated,
            redeemed        = false
        "#,
    )
    .bind(rav.collection_id)
    .bind(rav.payer_address)
    .bind(rav.service_provider)
    .bind(rav.data_service)
    .bind(rav.timestamp_ns)
    .bind(rav.value_aggregate)
    .bind(rav.signature)
    .bind(rav.last_updated)
    .execute(pool)
    .await?;
    Ok(())
}
