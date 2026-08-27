// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0
//
// M1 gate smoke test: CreateTable -> PutItem -> GetItem round-trip through the
// wasm dispatch, plus a miss (GetItem on absent key returns no Item).
// Run after: wasm-pack build crates/wasm --target nodejs
//   node crates/wasm/tests-node/smoke.cjs

const assert = require("node:assert");
const { init, dispatch } = require("../pkg/extenddb_wasm.js");

function call(target, obj) {
  const raw = dispatch(`DynamoDB_20120810.${target}`, JSON.stringify(obj));
  const res = JSON.parse(raw);
  if (res.__type) {
    throw new Error(`${target} failed: ${res.__type} ${res.message}`);
  }
  return res;
}

init();

// CreateTable (hash + range key).
const created = call("CreateTable", {
  TableName: "Music",
  KeySchema: [
    { AttributeName: "Artist", KeyType: "HASH" },
    { AttributeName: "SongTitle", KeyType: "RANGE" },
  ],
  AttributeDefinitions: [
    { AttributeName: "Artist", AttributeType: "S" },
    { AttributeName: "SongTitle", AttributeType: "S" },
  ],
  BillingMode: "PAY_PER_REQUEST",
});
assert.strictEqual(created.TableDescription.TableStatus, "ACTIVE");

// PutItem.
call("PutItem", {
  TableName: "Music",
  Item: {
    Artist: { S: "Radiohead" },
    SongTitle: { S: "Paranoid Android" },
    Album: { S: "OK Computer" },
    Year: { N: "1997" },
  },
});

// GetItem hit.
const got = call("GetItem", {
  TableName: "Music",
  Key: {
    Artist: { S: "Radiohead" },
    SongTitle: { S: "Paranoid Android" },
  },
});
assert.deepStrictEqual(got.Item.Album, { S: "OK Computer" });
assert.deepStrictEqual(got.Item.Year, { N: "1997" });

// GetItem miss.
const miss = call("GetItem", {
  TableName: "Music",
  Key: {
    Artist: { S: "Nobody" },
    SongTitle: { S: "Nothing" },
  },
});
assert.strictEqual(miss.Item, undefined);

// --- M2c: Scan (base table) with Limit + ExclusiveStartKey pagination ---

call("PutItem", {
  TableName: "Music",
  Item: { Artist: { S: "Radiohead" }, SongTitle: { S: "Karma Police" }, Album: { S: "OK Computer" } },
});
call("PutItem", {
  TableName: "Music",
  Item: { Artist: { S: "Muse" }, SongTitle: { S: "Uprising" }, Album: { S: "The Resistance" } },
});

const scanAll = call("Scan", { TableName: "Music" });
assert.strictEqual(scanAll.Items.length, 3);

const scanPage = call("Scan", { TableName: "Music", Limit: 2 });
assert.strictEqual(scanPage.Items.length, 2);
assert.ok(scanPage.LastEvaluatedKey, "limited scan should return LastEvaluatedKey");

const scanRest = call("Scan", {
  TableName: "Music",
  Limit: 2,
  ExclusiveStartKey: scanPage.LastEvaluatedKey,
});
assert.strictEqual(scanRest.Items.length, 1);

// --- M2c: Query (partition key + sort-key condition, typed ordering) ---

const q = call("Query", {
  TableName: "Music",
  KeyConditionExpression: "Artist = :a",
  ExpressionAttributeValues: { ":a": { S: "Radiohead" } },
});
assert.strictEqual(q.Items.length, 2);
assert.strictEqual(q.Items[0].SongTitle.S, "Karma Police"); // ascending sort-key order
assert.strictEqual(q.Items[1].SongTitle.S, "Paranoid Android");

const qDesc = call("Query", {
  TableName: "Music",
  KeyConditionExpression: "Artist = :a",
  ExpressionAttributeValues: { ":a": { S: "Radiohead" } },
  ScanIndexForward: false,
});
assert.strictEqual(qDesc.Items[0].SongTitle.S, "Paranoid Android"); // descending

const qBegins = call("Query", {
  TableName: "Music",
  KeyConditionExpression: "Artist = :a AND begins_with(SongTitle, :p)",
  ExpressionAttributeValues: { ":a": { S: "Radiohead" }, ":p": { S: "Karma" } },
});
assert.strictEqual(qBegins.Items.length, 1);
assert.strictEqual(qBegins.Items[0].SongTitle.S, "Karma Police");

const qRange = call("Query", {
  TableName: "Music",
  KeyConditionExpression: "Artist = :a AND SongTitle > :s",
  ExpressionAttributeValues: { ":a": { S: "Radiohead" }, ":s": { S: "Karma Police" } },
});
assert.strictEqual(qRange.Items.length, 1);
assert.strictEqual(qRange.Items[0].SongTitle.S, "Paranoid Android");

// --- M2c: UpdateItem (SET) + ConditionExpression on writes ---

const upd = call("UpdateItem", {
  TableName: "Music",
  Key: { Artist: { S: "Radiohead" }, SongTitle: { S: "Karma Police" } },
  UpdateExpression: "SET PlayCount = :pc",
  ExpressionAttributeValues: { ":pc": { N: "5" } },
  ReturnValues: "ALL_NEW",
});
assert.deepStrictEqual(upd.Attributes.PlayCount, { N: "5" });
assert.deepStrictEqual(upd.Attributes.Album, { S: "OK Computer" }); // untouched attrs preserved

// Conditional update that passes (Rating does not exist yet).
call("UpdateItem", {
  TableName: "Music",
  Key: { Artist: { S: "Radiohead" }, SongTitle: { S: "Karma Police" } },
  UpdateExpression: "SET Rating = :r",
  ConditionExpression: "attribute_not_exists(Rating)",
  ExpressionAttributeValues: { ":r": { N: "9" } },
});

// Conditional update that fails -> ConditionalCheckFailedException (no trap).
const condFail = JSON.parse(
  dispatch(
    "DynamoDB_20120810.UpdateItem",
    JSON.stringify({
      TableName: "Music",
      Key: { Artist: { S: "Radiohead" }, SongTitle: { S: "Karma Police" } },
      UpdateExpression: "SET Rating = :r",
      ConditionExpression: "attribute_not_exists(Rating)",
      ExpressionAttributeValues: { ":r": { N: "1" } },
    }),
  ),
);
assert.ok(
  String(condFail.__type).includes("ConditionalCheckFailedException"),
  "expected ConditionalCheckFailedException on update, got " + JSON.stringify(condFail),
);

// Conditional PutItem that fails because the item already exists.
const putFail = JSON.parse(
  dispatch(
    "DynamoDB_20120810.PutItem",
    JSON.stringify({
      TableName: "Music",
      Item: { Artist: { S: "Muse" }, SongTitle: { S: "Uprising" }, Album: { S: "x" } },
      ConditionExpression: "attribute_not_exists(Artist)",
    }),
  ),
);
assert.ok(
  String(putFail.__type).includes("ConditionalCheckFailedException"),
  "expected ConditionalCheckFailedException on put, got " + JSON.stringify(putFail),
);

// --- M2b: DescribeTable / ListTables / DeleteItem / DeleteTable ---

const desc = call("DescribeTable", { TableName: "Music" });
assert.strictEqual(desc.Table.TableName, "Music");
assert.strictEqual(desc.Table.TableStatus, "ACTIVE");
assert.strictEqual(desc.Table.KeySchema.length, 2);
// CreateTable(PAY_PER_REQUEST) and DescribeTable must agree (persisted column).
assert.strictEqual(desc.Table.BillingModeSummary.BillingMode, "PAY_PER_REQUEST");

const list = call("ListTables", {});
assert.ok(list.TableNames.includes("Music"), "ListTables should include Music");

// DeleteItem with ReturnValues=ALL_OLD returns the removed item.
const del = call("DeleteItem", {
  TableName: "Music",
  Key: { Artist: { S: "Radiohead" }, SongTitle: { S: "Paranoid Android" } },
  ReturnValues: "ALL_OLD",
});
assert.deepStrictEqual(del.Attributes.Album, { S: "OK Computer" });

const gone = call("GetItem", {
  TableName: "Music",
  Key: { Artist: { S: "Radiohead" }, SongTitle: { S: "Paranoid Android" } },
});
assert.strictEqual(gone.Item, undefined);

const delTable = call("DeleteTable", { TableName: "Music" });
assert.strictEqual(delTable.TableDescription.TableStatus, "DELETING");

// DescribeTable after delete should be a ResourceNotFoundException, not a trap.
const afterDelete = JSON.parse(
  dispatch("DynamoDB_20120810.DescribeTable", JSON.stringify({ TableName: "Music" })),
);
assert.ok(
  String(afterDelete.__type).includes("ResourceNotFoundException"),
  "expected ResourceNotFoundException after delete, got " + JSON.stringify(afterDelete),
);

// Deletion protection: persisted, reflected in Describe, and enforced on Delete.
call("CreateTable", {
  TableName: "Protected",
  KeySchema: [{ AttributeName: "id", KeyType: "HASH" }],
  AttributeDefinitions: [{ AttributeName: "id", AttributeType: "S" }],
  BillingMode: "PAY_PER_REQUEST",
  DeletionProtectionEnabled: true,
});
const protDesc = call("DescribeTable", { TableName: "Protected" });
assert.strictEqual(protDesc.Table.DeletionProtectionEnabled, true);
const protDel = JSON.parse(
  dispatch("DynamoDB_20120810.DeleteTable", JSON.stringify({ TableName: "Protected" })),
);
assert.ok(
  String(protDel.__type).includes("ValidationException"),
  "expected ValidationException deleting a protected table, got " + JSON.stringify(protDel),
);

// --- Pagination resume must survive a deleted ExclusiveStartKey (reviewer M2c #1) ---
call("CreateTable", {
  TableName: "Pager",
  KeySchema: [
    { AttributeName: "pk", KeyType: "HASH" },
    { AttributeName: "sk", KeyType: "RANGE" },
  ],
  AttributeDefinitions: [
    { AttributeName: "pk", AttributeType: "S" },
    { AttributeName: "sk", AttributeType: "N" },
  ],
  BillingMode: "PAY_PER_REQUEST",
});
for (const n of ["1", "2", "3"]) {
  call("PutItem", { TableName: "Pager", Item: { pk: { S: "p" }, sk: { N: n } } });
}

// Query: page 1 -> [1,2], delete the LEK row (sk=2), resume -> must return [3]
// (typed resume), not restart from the top (duplicates) or return empty.
const qp1 = call("Query", {
  TableName: "Pager",
  KeyConditionExpression: "pk = :p",
  ExpressionAttributeValues: { ":p": { S: "p" } },
  Limit: 2,
});
assert.deepStrictEqual(qp1.Items.map((i) => i.sk.N), ["1", "2"]);
call("DeleteItem", { TableName: "Pager", Key: { pk: { S: "p" }, sk: { N: "2" } } });
const qp2 = call("Query", {
  TableName: "Pager",
  KeyConditionExpression: "pk = :p",
  ExpressionAttributeValues: { ":p": { S: "p" } },
  Limit: 2,
  ExclusiveStartKey: qp1.LastEvaluatedKey,
});
assert.deepStrictEqual(qp2.Items.map((i) => i.sk.N), ["3"], "Query must resume past a deleted ESK");

// Scan: deleting the LEK row must not silently end the scan.
const sp1 = call("Scan", { TableName: "Pager", Limit: 1 });
assert.strictEqual(sp1.Items.length, 1);
assert.ok(sp1.LastEvaluatedKey, "limited scan returns LastEvaluatedKey");
call("DeleteItem", { TableName: "Pager", Key: sp1.LastEvaluatedKey });
const sp2 = call("Scan", { TableName: "Pager", ExclusiveStartKey: sp1.LastEvaluatedKey });
assert.ok(sp2.Items.length >= 1, "Scan must resume past a deleted ESK, not return an empty page");

// Item-size limit (400KB) enforced on UpdateItem's post-apply image.
const bigVal = "x".repeat(450 * 1024);
const tooBig = JSON.parse(
  dispatch(
    "DynamoDB_20120810.UpdateItem",
    JSON.stringify({
      TableName: "Pager",
      Key: { pk: { S: "big" }, sk: { N: "1" } },
      UpdateExpression: "SET Blob = :b",
      ExpressionAttributeValues: { ":b": { S: bigVal } },
    }),
  ),
);
assert.ok(
  String(tooBig.__type).includes("ValidationException"),
  "oversized UpdateItem should be rejected with ValidationException",
);

// --- Vector index + SearchVectors (engine path) ---

const vecCreated = call("CreateTable", {
  TableName: "Vecs",
  KeySchema: [{ AttributeName: "pk", KeyType: "HASH" }],
  AttributeDefinitions: [{ AttributeName: "pk", AttributeType: "S" }],
  BillingMode: "PAY_PER_REQUEST",
  VectorIndexes: [
    {
      IndexName: "vidx",
      Dimensions: 3,
      DistanceFunction: "COSINE",
      VectorAttribute: { AttributeName: "emb" },
      Projection: { ProjectionType: "ALL" },
    },
  ],
});
// The index is ACTIVE at birth: no control plane, nothing to backfill.
assert.strictEqual(vecCreated.TableDescription.VectorIndexes.length, 1);
assert.strictEqual(vecCreated.TableDescription.VectorIndexes[0].IndexStatus, "ACTIVE");
const vecDesc = call("DescribeTable", { TableName: "Vecs" });
assert.strictEqual(vecDesc.Table.VectorIndexes[0].IndexName, "vidx");
assert.strictEqual(vecDesc.Table.VectorIndexes[0].Dimensions, 3);

const embeddings = {
  east: ["1", "0", "0"],
  north: ["0", "1", "0"],
  northeastish: ["0.6", "0.8", "0"],
};
for (const [pk, comps] of Object.entries(embeddings)) {
  call("PutItem", {
    TableName: "Vecs",
    Item: { pk: { S: pk }, emb: { L: comps.map((n) => ({ N: n })) } },
  });
}
// An item without the vector attribute never enters the index.
call("PutItem", { TableName: "Vecs", Item: { pk: { S: "novec" } } });

// Nearest neighbours of [1,0,0], most similar first (cosine: smaller = closer).
const sv = call("SearchVectors", {
  TableName: "Vecs",
  IndexName: "vidx",
  SearchVector: [{ N: "1" }, { N: "0" }, { N: "0" }],
  TopK: 2,
});
assert.deepStrictEqual(
  sv.SearchResults.map((r) => r.Item.pk.S),
  ["east", "northeastish"],
);
// A self-match scores exactly 0 from above (the clamp), never negative.
assert.ok(sv.SearchResults[0].Score === 0, "self-match must score 0");
assert.ok(sv.SearchResults[1].Score > 0 && sv.SearchResults[1].Score < 1);
// The vector attribute is withheld unless a ProjectionExpression names it.
assert.strictEqual(sv.SearchResults[0].Item.emb, undefined);

// TopK larger than the item count returns every indexed item, ordered.
const svAll = call("SearchVectors", {
  TableName: "Vecs",
  IndexName: "vidx",
  SearchVector: [{ N: "1" }, { N: "0" }, { N: "0" }],
  TopK: 100,
});
assert.deepStrictEqual(
  svAll.SearchResults.map((r) => r.Item.pk.S),
  ["east", "northeastish", "north"],
  "TopK > item count must return all indexed items (novec excluded)",
);

// Naming the vector attribute in a ProjectionExpression returns it.
const svProj = call("SearchVectors", {
  TableName: "Vecs",
  IndexName: "vidx",
  SearchVector: [{ N: "1" }, { N: "0" }, { N: "0" }],
  TopK: 1,
  ProjectionExpression: "pk, emb",
});
assert.deepStrictEqual(svProj.SearchResults[0].Item.emb.L.map((v) => v.N), ["1", "0", "0"]);

// The write paths maintain the index: a delete leaves the index too.
call("DeleteItem", { TableName: "Vecs", Key: { pk: { S: "east" } } });
const svAfterDelete = call("SearchVectors", {
  TableName: "Vecs",
  IndexName: "vidx",
  SearchVector: [{ N: "1" }, { N: "0" }, { N: "0" }],
  TopK: 100,
});
assert.deepStrictEqual(
  svAfterDelete.SearchResults.map((r) => r.Item.pk.S),
  ["northeastish", "north"],
);

// And an UpdateItem re-ranks: north moves next to the query vector.
call("UpdateItem", {
  TableName: "Vecs",
  Key: { pk: { S: "north" } },
  UpdateExpression: "SET emb = :v",
  ExpressionAttributeValues: { ":v": { L: [{ N: "0.9" }, { N: "0.1" }, { N: "0" }] } },
});
const svAfterUpdate = call("SearchVectors", {
  TableName: "Vecs",
  IndexName: "vidx",
  SearchVector: [{ N: "1" }, { N: "0" }, { N: "0" }],
  TopK: 100,
});
assert.deepStrictEqual(
  svAfterUpdate.SearchResults.map((r) => r.Item.pk.S),
  ["north", "northeastish"],
);

// A wrong-dimension query is refused, not answered.
const svBadDims = JSON.parse(
  dispatch(
    "DynamoDB_20120810.SearchVectors",
    JSON.stringify({
      TableName: "Vecs",
      IndexName: "vidx",
      SearchVector: [{ N: "1" }, { N: "0" }],
      TopK: 1,
    }),
  ),
);
assert.ok(
  String(svBadDims.__type).includes("ValidationException"),
  "dimension mismatch must be a ValidationException, got " + JSON.stringify(svBadDims),
);

console.log(
  "M2c smoke test PASSED: Create/Describe/List, Put/Get, Update(SET)+Condition, Query(order/begins_with/range), Scan(+Limit/ESK), pagination-resume-after-delete, DeleteItem(ALL_OLD), DeleteTable, vector index + SearchVectors OK",
);
