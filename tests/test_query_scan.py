# Copyright 2026 ExtendDB contributors
# SPDX-License-Identifier: Apache-2.0

"""Phase 5 Query and Scan tests — dual-target against real DynamoDB and extenddb.

Covers: Query (KeyConditionExpression, FilterExpression, ProjectionExpression,
pagination, ScanIndexForward, Select=COUNT), Scan (FilterExpression,
ProjectionExpression, pagination, parallel scan), and error validation.
REQ-TEST-001, REQ-TEST-002, REQ-TEST-003
"""

from __future__ import annotations

import uuid

import pytest
from botocore.exceptions import ClientError

from conftest import wait_for_active
@pytest.fixture()
def query_table(dynamodb_client, create_and_cleanup_table):
    """Create a hash+range (S,N) table with 10 items for query tests."""
    name = f"extenddb-test-{uuid.uuid4().hex[:12]}"
    create_and_cleanup_table(
        name,
        AttributeDefinitions=[
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "sk", "AttributeType": "N"},
        ],
        KeySchema=[
            {"AttributeName": "pk", "KeyType": "HASH"},
            {"AttributeName": "sk", "KeyType": "RANGE"},
        ],
    )
    wait_for_active(dynamodb_client, name)
    for i in range(1, 11):
        dynamodb_client.put_item(
            TableName=name,
            Item={
                "pk": {"S": "user-1"},
                "sk": {"N": str(i)},
                "name": {"S": f"item-{i}"},
                "age": {"N": str(20 + i)},
            },
        )
    return name
@pytest.fixture()
def string_sk_table(dynamodb_client, create_and_cleanup_table):
    """Create a hash+range (S,S) table with items for begins_with tests."""
    name = f"extenddb-test-{uuid.uuid4().hex[:12]}"
    create_and_cleanup_table(
        name,
        AttributeDefinitions=[
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "sk", "AttributeType": "S"},
        ],
        KeySchema=[
            {"AttributeName": "pk", "KeyType": "HASH"},
            {"AttributeName": "sk", "KeyType": "RANGE"},
        ],
    )
    wait_for_active(dynamodb_client, name)
    items = ["alpha-1", "alpha-2", "beta-1", "gamma-1"]
    for prefix in items:
        dynamodb_client.put_item(
            TableName=name,
            Item={"pk": {"S": "user-1"}, "sk": {"S": prefix}, "data": {"S": "v"}},
        )
    # Verify all items are visible before yielding to tests.
    resp = dynamodb_client.query(
        TableName=name,
        KeyConditionExpression="pk = :pk",
        ExpressionAttributeValues={":pk": {"S": "user-1"}},
        ConsistentRead=True,
    )
    assert resp["Count"] == len(items), (
        f"string_sk_table fixture: expected {len(items)} items, got {resp['Count']}"
    )
    return name
@pytest.fixture()
def scan_table(dynamodb_client, create_and_cleanup_table):
    """Create a hash-only table with 13 items for scan tests."""
    name = f"extenddb-test-{uuid.uuid4().hex[:12]}"
    create_and_cleanup_table(name)
    wait_for_active(dynamodb_client, name)
    for i in range(1, 14):
        dynamodb_client.put_item(
            TableName=name,
            Item={
                "pk": {"S": f"item-{i:03d}"},
                "category": {"S": "a" if i <= 3 else "b"},
            },
        )
    return name
class TestQuery:
    """Query operation tests."""

    def test_query_pk_only(self, dynamodb_client, query_table):
        resp = dynamodb_client.query(
            TableName=query_table,
            KeyConditionExpression="pk = :pk",
            ExpressionAttributeValues={":pk": {"S": "user-1"}},
        )
        assert resp["Count"] == 10
        assert resp["ScannedCount"] == 10

    def test_query_sk_eq(self, dynamodb_client, query_table):
        resp = dynamodb_client.query(
            TableName=query_table,
            KeyConditionExpression="pk = :pk AND sk = :sk",
            ExpressionAttributeValues={":pk": {"S": "user-1"}, ":sk": {"N": "5"}},
        )
        assert resp["Count"] == 1
        assert resp["Items"][0]["name"] == {"S": "item-5"}

    def test_query_sk_lt(self, dynamodb_client, query_table):
        resp = dynamodb_client.query(
            TableName=query_table,
            KeyConditionExpression="pk = :pk AND sk < :sk",
            ExpressionAttributeValues={":pk": {"S": "user-1"}, ":sk": {"N": "4"}},
        )
        assert resp["Count"] == 3

    def test_query_sk_between(self, dynamodb_client, query_table):
        resp = dynamodb_client.query(
            TableName=query_table,
            KeyConditionExpression="pk = :pk AND sk BETWEEN :lo AND :hi",
            ExpressionAttributeValues={
                ":pk": {"S": "user-1"},
                ":lo": {"N": "3"},
                ":hi": {"N": "7"},
            },
        )
        assert resp["Count"] == 5

    def test_query_begins_with(self, dynamodb_client, string_sk_table):
        resp = dynamodb_client.query(
            TableName=string_sk_table,
            KeyConditionExpression="pk = :pk AND begins_with(sk, :prefix)",
            ExpressionAttributeValues={
                ":pk": {"S": "user-1"},
                ":prefix": {"S": "alpha"},
            },
        )
        assert resp["Count"] == 2

    def test_query_reverse_order(self, dynamodb_client, query_table):
        resp = dynamodb_client.query(
            TableName=query_table,
            KeyConditionExpression="pk = :pk",
            ExpressionAttributeValues={":pk": {"S": "user-1"}},
            ScanIndexForward=False,
        )
        sks = [int(item["sk"]["N"]) for item in resp["Items"]]
        assert sks == list(range(10, 0, -1))

    def test_query_limit(self, dynamodb_client, query_table):
        resp = dynamodb_client.query(
            TableName=query_table,
            KeyConditionExpression="pk = :pk",
            ExpressionAttributeValues={":pk": {"S": "user-1"}},
            Limit=3,
        )
        assert resp["Count"] == 3
        assert "LastEvaluatedKey" in resp

    def test_query_pagination(self, dynamodb_client, query_table):
        all_items: list = []
        kwargs: dict = {
            "TableName": query_table,
            "KeyConditionExpression": "pk = :pk",
            "ExpressionAttributeValues": {":pk": {"S": "user-1"}},
            "Limit": 3,
        }
        while True:
            resp = dynamodb_client.query(**kwargs)
            all_items.extend(resp["Items"])
            if "LastEvaluatedKey" not in resp:
                break
            kwargs["ExclusiveStartKey"] = resp["LastEvaluatedKey"]
        assert len(all_items) == 10

    def test_query_filter_expression(self, dynamodb_client, query_table):
        resp = dynamodb_client.query(
            TableName=query_table,
            KeyConditionExpression="pk = :pk",
            FilterExpression="age > :min_age",
            ExpressionAttributeValues={
                ":pk": {"S": "user-1"},
                ":min_age": {"N": "25"},
            },
        )
        assert resp["Count"] == 5
        assert resp["ScannedCount"] == 10

    def test_query_projection(self, dynamodb_client, query_table):
        resp = dynamodb_client.query(
            TableName=query_table,
            KeyConditionExpression="pk = :pk AND sk = :sk",
            ExpressionAttributeValues={":pk": {"S": "user-1"}, ":sk": {"N": "1"}},
            ProjectionExpression="#n",
            ExpressionAttributeNames={"#n": "name"},
        )
        item = resp["Items"][0]
        # Only projected attributes returned — keys not auto-included
        assert "name" in item
        assert "pk" not in item
        assert "sk" not in item
        assert "age" not in item

    def test_query_select_count(self, dynamodb_client, query_table):
        resp = dynamodb_client.query(
            TableName=query_table,
            KeyConditionExpression="pk = :pk",
            ExpressionAttributeValues={":pk": {"S": "user-1"}},
            Select="COUNT",
        )
        assert resp["Count"] == 10
        assert "Items" not in resp or resp["Items"] is None

    def test_query_select_count_with_filter(self, dynamodb_client, query_table):
        """COUNT with FilterExpression: Count = filtered items, ScannedCount = total read."""
        resp = dynamodb_client.query(
            TableName=query_table,
            KeyConditionExpression="pk = :pk",
            FilterExpression="age > :min_age",
            ExpressionAttributeValues={
                ":pk": {"S": "user-1"},
                ":min_age": {"N": "25"},
            },
            Select="COUNT",
        )
        assert resp["Count"] == 5
        assert resp["ScannedCount"] == 10

    def test_query_missing_kce(self, dynamodb_client, query_table):
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.query(TableName=query_table)
        assert exc_info.value.response["Error"]["Code"] == "ValidationException"

    def test_query_ne_operator_rejected_in_kce(self, dynamodb_client, query_table):
        """DynamoDB rejects <> in KeyConditionExpression (only =, <, <=, >, >=, BETWEEN, begins_with)."""
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.query(
                TableName=query_table,
                KeyConditionExpression="pk = :pk AND sk <> :val",
                ExpressionAttributeValues={
                    ":pk": {"S": "user-1"},
                    ":val": {"S": "x"},
                },
            )
        assert exc_info.value.response["Error"]["Code"] == "ValidationException"
class TestScan:
    """Scan operation tests."""

    def test_scan_full_table(self, dynamodb_client, scan_table):
        resp = dynamodb_client.scan(TableName=scan_table)
        assert resp["Count"] == 13

    def test_scan_filter_expression(self, dynamodb_client, scan_table):
        resp = dynamodb_client.scan(
            TableName=scan_table,
            FilterExpression="category = :cat",
            ExpressionAttributeValues={":cat": {"S": "a"}},
        )
        assert resp["Count"] == 3

    def test_scan_limit(self, dynamodb_client, scan_table):
        resp = dynamodb_client.scan(TableName=scan_table, Limit=5)
        assert resp["Count"] == 5
        assert "LastEvaluatedKey" in resp

    def test_scan_pagination(self, dynamodb_client, scan_table):
        all_items: list = []
        kwargs: dict = {"TableName": scan_table, "Limit": 4}
        while True:
            resp = dynamodb_client.scan(**kwargs)
            all_items.extend(resp["Items"])
            if "LastEvaluatedKey" not in resp:
                break
            kwargs["ExclusiveStartKey"] = resp["LastEvaluatedKey"]
        assert len(all_items) == 13

    def test_scan_projection(self, dynamodb_client, scan_table):
        resp = dynamodb_client.scan(
            TableName=scan_table,
            ProjectionExpression="pk",
            Limit=1,
        )
        item = resp["Items"][0]
        assert "pk" in item
        assert "category" not in item

    def test_scan_parallel(self, dynamodb_client, scan_table):
        total_segments = 3
        all_items: list = []
        for seg in range(total_segments):
            resp = dynamodb_client.scan(
                TableName=scan_table,
                Segment=seg,
                TotalSegments=total_segments,
            )
            all_items.extend(resp["Items"])
        assert len(all_items) == 13

    def test_scan_segment_without_total(self, dynamodb_client, scan_table):
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.scan(TableName=scan_table, Segment=0)
        assert exc_info.value.response["Error"]["Code"] == "ValidationException"

    def test_scan_total_without_segment(self, dynamodb_client, scan_table):
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.scan(TableName=scan_table, TotalSegments=3)
        assert exc_info.value.response["Error"]["Code"] == "ValidationException"
