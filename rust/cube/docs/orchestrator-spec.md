# Query orchestrator — specification for the Rust port

Derived from `packages/cubejs-query-orchestrator`, `packages/cubejs-cubestore-driver`,
`packages/cubejs-api-gateway` and `packages/cubejs-server-core` (paths below use
`QO/` = `packages/cubejs-query-orchestrator/src/orchestrator/`, `CSD/` =
`packages/cubejs-cubestore-driver/src/`, `GW/` = `packages/cubejs-api-gateway/src/`,
`SC/` = `packages/cubejs-server-core/src/core/`). Part of the Node.js → Rust
migration, see `../MIGRATION.md` (workstreams 3 and 4).

## 1. fetchQuery state machine and result shape

Entry `QueryOrchestrator.fetchQuery(queryBody)` — `QO/QueryOrchestrator.ts:212-291`.

1. `preAggregations.loadAllPreAggregationsIfNeeded(queryBody)` (`QO/PreAggregations.ts:529-632`) runs pre-agg descriptions **sequentially** (reduce over promises, :626) producing `PreAggTableToTempTable = [tableName, LoadPreAggregationResult]` and optional `values` replacement (`BUILD_RANGE_START_LOCAL`/`BUILD_RANGE_END_LOCAL`, `QO/PreAggregationPartitionRangeLoader.ts:144-160`). Each result is stamped with `preAggregationId`, `type`, `dataSource`, `timezone` (:593-599) and `addTableUsed(targetTableName)` unless `isMultiTableUnion` (:600-602). `usageTargetTableNames` expands one entry into `tableName+suffix` entries (:613-625).
2. `usedPreAggregations` = map `tableName -> { targetTableName, refreshKeyValues, lastUpdatedAt, preAggregationId, type }` (`QueryOrchestrator.ts:225-238`).
3. `rollupOnlyMode && usedPreAggregations empty` → throw `"No pre-aggregation table has been built for this query yet. …"` (:240-245).
4. `lastRefreshTimestamp = min(nonNull lastUpdatedAt)` — `getLastUpdatedAtTimestamp` returns the **oldest** (`PreAggregations.ts:63-72`).
5. No `queryBody.query` (build-only): if `isJob` → array of `{ preAggregation, tableName, ...loadResult }` (:255-260); else `{ usedPreAggregations, lastRefreshTime }` (:261-266).
6. `queryCache.cachedQueryResult(queryBody, preAggTables)` (`QO/QueryCache.ts:278-457`):
   - SQL rewritten via single-pass longest-first regex replacement of pre-agg table names (`:545-571`).
   - `queuePriority = queryBody.queuePriority ?? QueuePriority.Interactive(10)` (:293-297).
   - `forceNoCache = queryBody.forceNoCache || cacheMode === 'no-cache'` (:299).
   - `expireSecs = queryBody.expireSecs || 86400` (:459-461); `cacheKeyQueries` = refresh-key queries with pre-agg names replaced (:303-308); `renewalThreshold = queryBody.cacheKeyQueries?.renewalThreshold`.
   - Branch A — no cacheKeyQueries, or (`external && skipExternalCacheAndQueue`), or `persistent` (:316-355): run through queue directly; persistent → returns `QueryStream`; else `{ data }` with cacheKey `[query, values]`.
   - Branch B — `cacheMode === 'must-revalidate'` → `renewQuery(..., skipRefreshKeyWaitForRenew: true)` (:357-376).
   - Branch C — default (`!backgroundRenew && cacheMode !== 'stale-while-revalidate'`) → foreground `renewQuery` then a detached `startRenewCycle` at Background priority (:378-417).
   - Branch D — background renew → `cacheQueryResult` + optional renew cycle, returns `{ data, lastRefreshTime }` (:419-456).
   - `renewQuery` (:934-996) = `Promise.all(loadRefreshKeys)` → `cacheQueryResult(query, values, cacheKey, expireSecs, { renewalThreshold: renewalThreshold || 6*3600, renewalKey: [cacheKeyQueries, results, hash([query,values])], waitForRenew: true, … })` and returns `{ data, refreshKeyValues, lastRefreshTime }`.
   - Cache decision `decideCacheAction` (:1039-1069): expired = `!renewalThreshold || !entry.time || renewedAgo > renewalThreshold*1000`; keyMismatch = `renewalKey && entry.renewalKey !== renewalKey`. Fresh+match → ServeCached. Same request UUID (`extractRequestUUID`) and not `renewCycle` → RefreshSameRequest (background). No `renewalKey` → ServeCached. Else `waitForRenew ? WaitForRenew(blocking fetch) : RefreshBackground`.
   - In-memory LRU (`max = maxInMemoryCacheEntries || 10000`, :230-232) usable only if `renewedAgo <= expiration*1000`, `<= 5min` (`IN_MEMORY_CACHE_DISABLE_PERIOD`, :203) and `renewedAgo + 5min <= renewalThreshold*1000` with matching renewalKey (:1071-1091).
7. `lastRefreshTime = min(preAgg lastUpdatedAt, cache entry time)` (`QueryOrchestrator.ts:274-277`); `QueryStream` returned as-is (:279-282).

**Result handed to the gateway** (`QueryOrchestrator.ts:284-290` + `SC/OrchestratorApi.ts:118-119`):
```
{ data, refreshKeyValues?, lastRefreshTime?: Date, dataSource, external, usedPreAggregations,
  dbType: await contextToDbType(dataSource), extDbType: contextToExternalDbType(),
  slowQuery?: true (only on the continue-wait/stale path, OrchestratorApi.ts:151-154),
  total? (set by gateway) }
```
Gateway shapes the HTTP body in `prepareResultTransformData` (`GW/gateway.ts:2050-2076`): `lastRefreshTime` → ISO string; `usedPreAggregations` redacted to `{preAggregationId,lastUpdatedAt,type}` outside dev/playground (`:155-195`), omitted when empty (`:142-152`); dev mode adds `refreshKeyValues`, full `usedPreAggregations`, `transformedQuery`, `requestId`.

## 2. Cache-key and queue-key algorithms

- `getCacheHash(queryKey, processUid)` (`QO/utils.ts:15-33`): if `typeof key === 'string' && len < 256` → key verbatim; else `md5(JSON.stringify(key)).hex()`; if the key object carries `persistent === true` → `"<md5>@<processUid>"`. `BaseQueueDriver.redisHash` delegates to it (`QO/BaseQueueDriver.ts:12-14`); `CubeStoreQueueDriver.hashQueryKey` (`CSD/CubeStoreQueueDriver.ts:23-32`) is the same minus the short-string shortcut.
- Query cache key: `QueryCache.queryCacheKey(queryBody)` = `[query, values, preAggregations.map(p => p.loadSql)]` (+ `invalidate` when present), with a non-enumerable `persistent` flag (`QO/QueryCache.ts:469-481`). Cache key string = `` `${cachePrefix}#SQL_QUERY_RESULT:${getCacheHash(key)}` `` (`:266-268, :1301-1303`).
- Refresh-key identity: `[sql, params, !!options.external, dataSource||'default']` (`:490-498`); cache key = same hash pipeline (`:537-539`). `buildRangeInvalidateKey` = identity of `invalidateKeyQueries[0]` (`:506-511`).
- Pre-agg queue key: `[loadSql, indexesSql?, invalidationKeys]` (`QO/PreAggregationLoader.ts:455-459`); stage key = `preAggregation.tableName` (`PreAggregations.ts:806-808`). `queryKeyMd5` used for logging/driver query options special-cases `FETCH_TABLES_FOR` (`PreAggregationLoader.ts:55-72`).
- Other cache namespaces (`PreAggregations.ts:340-358`): `SQL_PRE_AGGREGATIONS_TABLES_USED`, `..._TABLES_TOUCH`, `SQL_PRE_AGGREGATIONS_BACKOFF`, `..._REFRESH_END_REACHED`, `SQL_PRE_AGGREGATIONS_TABLES:<dataSource><schema>[_EXT]` (`PreAggregationLoadCache.ts:108-110`), `lock:<key>` for `withLock`.
- Queue prefixes: `SQL_QUERY_${prefix}_${dataSource}` (`QueryCache.ts:670`), `SQL_QUERY_EXT_${prefix}` (:739), `SQL_PRE_AGGREGATIONS_${prefix}_${ds}` (`PreAggregations.ts:724`), `SQL_PRE_AGGREGATIONS_CACHE_${prefix}_${ds}` (:775). CubeStore path = `"<prefix>:<hash>"` (`CSD/CubeStoreQueueDriver.ts:91-93`).

## 3. Queue semantics and CubeStore command protocol

`QueryQueue` (`QO/QueryQueue.ts`): defaults `concurrency 2`, `continueWaitTimeout 10s`, `executionTimeout = getEnv('dbQueryTimeout')`, `orphanedTimeout 120s`, `heartBeatInterval 30s`, `heartBeatTimeout = heartBeatInterval*4` (:124-149). Priority must be in `[-10000, 10000]` (:245-247). Enum: Interactive 10, Warmup 1, Background 0, Scheduled -1 (`packages/cubejs-base-driver/src/queue-driver.interface.ts:32-41`).

`executeInQueue(handler, queryKey, query, priority, opts)` (:198-363): `skipQueue` → run inline (:214-238). Else: `getResult` → return if present; `forceBuild` → skip lookup and bail if a def exists; `addToQueue`; subscribe to stream event when handler==='stream'; `dispatchQuery` if fast-track retrieved else `reconcileQueue`; `getResultBlocking`; falsy result and `!isJob` → `throw ContinueWaitError`. `parseResult` (:418-432) raises `Error(result.error)` or returns `result.result`.

`reconcileQueueImpl` (:582-640): `getQueriesToCancel` → `getQueryAndRemove` + cancel handler; `getActiveAndToProcess`; `toProcessLimit = active.length >= concurrency ? 1 : concurrency - active.length`; skip persistent keys whose `@processUid` suffix is not this process; `processQuery` per pick (awaits retrieval, not execution). `executeQuery` (:849-1070): `MERGE_EXTRA {startQueryTime}`, heartbeat interval timer (also detects external cancellation by a missing def, :905-932), handler under `queryTimeout(executionTimeout)`, then `setResultAndRemoveQuery`, then `reconcileQueue`.

CubeStore SQL commands (`CSD/CubeStoreQueueDriver.ts`; response shapes documented in `packages/cubejs-query-orchestrator/DEVELOPMENT.md:22-88`):

| Op | SQL | Args | Response |
|---|---|---|---|
| add | `QUEUE ADD[ EXCLUSIVE] PRIORITY ?[ ORPHANED ?][ EXTERNAL_ID ?] ? ?` (:130-135, :152) | priority, [orphanedTimeout], [externalId], path, JSON payload | `{id, added:'true'/'false', pending}` |
| add+retrieve | `QUEUE ADD_AND_RETRIEVE … ?` (+concurrency) (:146-152) | … , concurrency | add fields + `{active, payload, extra}` |
| retrieve | `QUEUE RETRIEVE CONCURRENCY ? ?` (:366-376) | concurrency, path | `{id, active(csv|NULL), pending, payload, extra}`; 0 rows = failed |
| get def | `QUEUE GET ?` (:322-331) | queueId ?? path | `{payload, extra}` |
| result | `QUEUE RESULT [EXTERNAL_ID ? ]?` (:256-272) | [externalId], path | `{payload, type, id, external_id}` |
| blocking | `QUEUE RESULT_BLOCKING ? ?` (:378-389) | continueWaitTimeout*1000 ms, queueId ?? path | same or 0 rows on timeout |
| ack | `QUEUE ACK ? ?` (:391-405) | queueId ?? path, JSON result or NULL | `{success:'true'/'false'}` |
| heartbeat | `QUEUE HEARTBEAT ?` (:407-412) | queueId ?? path | — |
| cancel | `QUEUE CANCEL ?` (:171-181) | queueId ?? path | `{payload, extra}` |
| merge extra | `QUEUE MERGE_EXTRA ? ?` (:333-341) | queueId ?? path, JSON patch | — |
| list | `QUEUE LIST [WITH_PAYLOAD ]?` (:207, :233) | prefix | `{id(path), queue_id, status:'pending'\|'active', extra, payload?}` |
| active / pending | `QUEUE ACTIVE ?` / `QUEUE PENDING ?` (:184, :194) | prefix | list rows |
| stalled | `QUEUE STALLED ? ?` (:274-283) | heartBeatTimeout*1000, prefix | `{id, queue_id}` |
| orphaned | `QUEUE ORPHANED ? ?` (:285-294) | orphanedTimeout*1000, prefix | `{id, queue_id}` |
| to cancel | `QUEUE TO_CANCEL ? ? ?` (:296-306) | heartBeatTimeout*1000, orphanedTimeout*1000, prefix | `{id, queue_id}` |

Def decoding: `JSON.parse(payload)` merged with `JSON.parse(extra)` (:308-320). `EXTERNAL_ID` gated by `CUBEJS_QUEUE_EXTERNAL_ID` + driver capability; fast-track by `CUBEJS_QUEUE_FAST_TRACK` + `queueAddAndRetrieve` and only for priority ≥ Interactive (:71-85). Payload carries `{queryHandler, query, queryKey, stageQueryKey, priority, requestId, addedToQueueTime}` (:102-110).

Cache commands (`CSD/CubeStoreCacheDriver.ts`): `CACHE SET TTL ? ? ?` (ttlSecs, key, JSON), `CACHE GET ?` → `rows[0].value` JSON, `CACHE REMOVE ?`, `CACHE KEYS ?` (prefix) → `row.key`, `CACHE SET NX TTL ? ? ?` with `'1'` for `withLock` (success `rows[0].success === 'true'`). Params are either bound (when `CUBEJS_CUBESTORE_SENDABLE_PARAMETERS=true` and the server advertises `sendableParameters`) or inlined by `formatSql` client-side (`CSD/CubeStoreDriver.ts:109-122`).

## 4. Pre-aggregation build algorithm (pseudo-code)

```
structure_version(pa)  = version([structureVersionLoadSql||loadSql, indexesSql?, streamOffset?, outputColumnTypes?])   # PreAggregations.ts:74-87
content_version(pa,IK) = version([... same ..., IK])                                                                   # PreAggregationLoader.ts:376-389
version(key) = base32('abcdefghijklmnopqrstuvwxyz012345') over first 5 bytes of md5(JSON(key))                         # PreAggregations.ts:28-60
target_table(v) = naming_version==2 ? `${table}_${content}_${structure}_${base32(floor(ms/1000))}`
                                    : `${table}_${content}_${structure}_${ms}`                                         # :810-816
parse back: /(.+)_(.+)_(.+)_(.+)/ ; 4th group <13 chars → base32 ts & naming_version 2                                  # tablesToVersionEntries :218-246

loadPreAggregations(pa):                                                       # PreAggregationPartitionRangeLoader.ts:249-399
  if pa.partitionGranularity and not pa.expandedPartition:
    buildRange = loadBuildRange()                                              # :487-517, cached range queries :93-118
      startDate,endDate = extractDate(cachedRangeQuery(q)) for preAggregationStartEndQueries
      snap to timeSeriesBoundaries(granularity), re-run range queries on first/last partition
      empty → now() in pa.timezone
    dateRange = intersect(buildRange, matchedTimeDimensionDateRange) or [buildRange[1],buildRange[1]]   # :458-467
    ranges = timeSeries(granularity, dateRange, timestampPrecision); assert len <= maxPartitions        # :469-482
    per range: descr = partitionPreAggregationDescription(range, buildRange)                            # :206-247
       tableName = base + dateRange[0][0..10|13|16] with [-T:] stripped                                  # :584-604
       loadRange[1] = min(range[1], buildRangeEnd); FROM/TO_PARTITION_RANGE params → UTC bounds          # :173-204
       incremental refresh keys get renewalThreshold shrunk near the update window boundary
       sealAt = loadRange[1] + updateWindowSeconds
    load each partition (loadPreAggregation(false)), externalRefresh fallback to ignore matched range    # :273-287
    targetTableName = single table or "(SELECT * FROM t1 UNION ALL …)"                                   # :321-325
    return { targetTableName, refreshKeyValues[], lastUpdatedAt=min, buildRangeEnd, lambdaTable?, isMultiTableUnion, usageTargetTableNames }
  else: PreAggregationLoader(pa).loadPreAggregation(true)

loadPreAggregation(throwOnMissingPartition):                                   # PreAggregationLoader.ts:132-210
  if isJob or (!externalRefresh and (waitForRenew or all invalidateKeyQueries already memoized)):
      r = loadPreAggregationWithKeys(); return {...r, refreshKeyValues: getInvalidationKeyValues(), queryKey: isJob?…:undefined}
  else:
      ve = versionEntries.byStructure[`${table}_${structure_version}`]
      externalRefresh: ve ? serve it : (throwOnMissingPartition ? throw noPreAggregationPartitionsBuiltMessage : null)
      ve present → serve it and kick off a background build; else build synchronously

loadPreAggregationWithKeys():                                                  # :212-366
  IK = partitionInvalidateKeyQueries ?? invalidateKeyQueries, each via loadCache.keyQueryResult (cached 1h)
  cv = content_version(pa, IK); sv = structure_version(pa); VE = loadCache.getVersionEntries(pa)
  byContent[table_cv] and !forceBuild            → serve (touch table)
  !waitForRenew && !forceBuild && byStructure hit→ serve (touch table)
  createSchemaIfNotExists when no version entries at all
  newVersionEntry = {table_name, structure_version: sv, content_version: cv, last_updated_at: client.nowTimestamp(), naming_version: 2}
  forceBuild            → executeInQueue(Interactive) [isJob: fire-and-forget]; then mostRecentResult()
  structure changed     → executeInQueue(Interactive); mostRecentResult()
  content changed       → waitForRenew ? executeInQueue(Background)+mostRecentResult() : scheduleRefresh(background)
  no version entry      → executeInQueue(Interactive); mostRecentResult()

refresh(newVersionEntry, IK, client):                                          # :466-522
  strategy = external ? (readOnly ? refreshReadOnlyExternalStrategy : refreshWriteStrategy) : refreshStoreInSourceStrategy
  store-in-source  : loadPreAggregationIntoTable(target, loadSql with names replaced, params, {streamOffset,outputColumnTypes,…});
                     createIndexes; fetchTables; finally dropOrphanedTables                              # :542-580
  write strategy   : [optional temp table via loadPreAggregationIntoTable] → unload/stream/downloadTable
                     → uploadTableWithIndexes(target, types, data, indexesSql, uniqueKeyColumns, {aggregationsColumns,createTableIndexes,sealAt})
                     → fetchTables → dropOrphanedTables(external=true) → cleanup temp table under a lock   # :606-965
  read-only external: unloadFromQuery / downloadQueryResults on pa.sql, then the same upload             # :729-792
  failure          : removeTableTouched(target) [+ removeTableUsed when not external]                     # :497-519

dropOrphanedTables(client, justCreated, external):                             # :1016-1083
  lock key = external ? 'drop-orphaned-tables-external' : `drop-orphaned-tables:${dataSource}` (TTL 5 min)
  keep = tablesUsed ∪ ( dropPreAggregationsWithoutTouch && refreshEndReached
                        ? tablesTouched
                        : latest per table_name ∪ latest per (table_name,structure_version) within structureVersionPersistTime )
         ∪ {justCreated}; drop everything else in the schema
```
Version-entry filtering: non-external pre-aggs exclude tables whose target name matches a def currently in the build queue (`PreAggregationLoadCache.ts:142-156`). `getVersionEntries` is memoized per `tablesCachePrefixKey` and guarded by `tablePrefixes` (:179-191).

## 5. "Continue wait" contract

- `ContinueWaitError` message is literally `"Continue wait"` (`QO/ContinueWaitError.ts`). Raised when `getResultBlocking` returns nothing within `continueWaitTimeout` (`QueryQueue.ts:345-351`).
- `OrchestratorApi.executeQuery` additionally wraps the whole `fetchQuery` in `pt.timeout(continueWaitTimeout*1000)` (`SC/OrchestratorApi.ts:96`); both that timeout and `ContinueWaitError` are converted (:122-161) to: `scheduledRefresh` → `{error:'Continue wait', stage:null}`; `cacheMode ∈ {stale-if-slow, stale-while-revalidate}` with a cached value → `{...fromCache, slowQuery:true}`; otherwise `{error:'Continue wait', stage: await queryStage(query)}`.
- `queryStage` (`QueryOrchestrator.ts:297-343`) reports either `"Building pre-aggregation i/n: #k in queue"` / `"…: Executing query"` or the SQL-queue stage (`{stage:'Executing query', timeElapsed}` / `{stage:'#k in queue'}`, `QueryQueue.ts:688-712`).
- Gateway returns **HTTP 200** with `{error: "Continue wait", requestId?}` (`GW/gateway.ts:2577-2586`). Clients must re-issue the identical request; the same `requestId` UUID makes `decideCacheAction` take `RefreshSameRequest` so polling converges (`QueryCache.ts:1053-1060`).
- Default `continueWaitTimeout = 10` s, validated `0..90` (`SC/optionsValidate.ts:16,125`).

## 6. Environment variables

| Var | Default | Effect |
|---|---|---|
| `CUBEJS_CACHE_AND_QUEUE_DRIVER` | none → `cubestore` if `NODE_ENV=production`, else `memory` (`QueryOrchestrator.ts:41-60`) | only `memory`/`cubestore` accepted |
| `CUBEJS_CUBESTORE_HOST` / `_PORT` / `_USER` / `_PASS` | — | Cube Store connection (`env.ts:1925-1932`) |
| `CUBEJS_CUBESTORE_MAX_CONNECT_RETRIES` / `_NO_HEART_BEAT_TIMEOUT` / `_MAX_MESSAGE_SIZE` | 20 / 30 s / 100 MiB (`:1933-1951`) | transport |
| `CUBEJS_CUBESTORE_SENDABLE_PARAMETERS` | `true` (`:2075`) | bind params vs inline |
| `CUBEJS_CONCURRENCY` (per data source) | unset → driver default → 5 (`env.ts:309-313`, `SC/OptsHandler.ts:231-279`) | queue concurrency |
| `CUBEJS_REFRESH_WORKER_CONCURRENCY` | unset (`:282`) | pre-agg queue concurrency |
| `CUBEJS_DB_QUERY_TIMEOUT` | `10m` (`:740-751`) | `executionTimeout` |
| `CUBEJS_DB_QUERY_STREAM_HIGH_WATER_MARK` | 8192 (`:778`) | stream buffering |
| `CUBEJS_ROLLUP_ONLY` | `false` (`:255`) | `rollupOnlyMode` |
| `CUBEJS_EXTERNAL_DEFAULT` | `true` (`:2078`) | warn/require Cube Store as external |
| `CUBEJS_QUEUE_EXTERNAL_ID` / `CUBEJS_QUEUE_FAST_TRACK` | `false` / `false` (`:2081-2082`) | protocol extensions |
| `CUBEJS_REFRESH_WORKER` (legacy `CUBEJS_SCHEDULED_REFRESH`, `_TIMER`) | `NODE_ENV !== 'production'` (`:261-280`) | run refresh loop |
| `CUBEJS_SCHEDULED_REFRESH_TIMEZONES` / `_QUERIES_PER_APP_ID` / `_BATCH_SIZE` / `_DEFAULT` | `[]` / unset / 1 / `true` (`:281-323, 2083-2085`) | scheduler scope; interval defaults to 30 s (`SC/OptsHandler.ts:573-586`) |
| `CUBEJS_PRE_AGGREGATIONS_SCHEMA` / `_BUILDER` / `_BACKOFF_MAX_TIME` / `_ALLOW_NON_STRICT_DATE_RANGE_MATCH` | — / — / 600 s / — (`:304, 318, 828, 373`) | pre-agg schema, builder role, backoff |
| `CUBEJS_MAX_PARTITIONS_PER_CUBE` / `CUBEJS_MAX_SOURCE_ROW_LIMIT` | 10000 / 200000 (`:320, 2094`) | partition/lambda limits |
| `CUBEJS_TOUCH_PRE_AGG_TIMEOUT` / `_CACHE_MAX_AGE` / `_CACHE_MAX_COUNT` / `CUBEJS_USED_PRE_AGG_CACHE_MAX_COUNT` / `CUBEJS_DROP_PRE_AGG_WITHOUT_TOUCH` | 86400 / half of it / 8192 / 8192 / `true` (`:785-836`) | orphan GC |
| `CUBEJS_REFRESH_KEY_LOCAL_TIME` | `false` (`:403`) | evaluate `every` refresh keys locally (`QO/utils.ts:45-53`) |

`continueWaitTimeout`, `orphanedTimeout`, `heartBeatInterval`, `refreshKeyRenewalThreshold`, `maxInMemoryCacheEntries` are config-only (`SC/types.ts:16-78`).

## 7. Proposed Rust layout and plan

```
rust/cube/
  cubecache/     CacheDriver trait {get,set,remove,keys_starting_with,with_lock,test_connection}
                 impls: MemoryCacheDriver (DashMap+TTL), CubeStoreCacheDriver (CACHE *)
                 cache_key.rs: CacheKey enum + get_cache_hash (md5 over canonical JSON — must match
                 serde_json field order of the JS object literals exactly)
  cubequeue/     QueueDriver + QueueDriverConnection traits mirroring base-driver's interface,
                 QueryQueue engine (reconcile, dispatch, heartbeat, timeout, cancel, stages),
                 impls: LocalQueueDriver, CubeStoreQueueDriver (QUEUE * command builders + row decoders)
  cubepreaggs/   version/content hashing, table naming, VersionEntry parsing, PreAggregationLoader,
                 PartitionRangeLoader (needs a timeSeries/timezone port of @cubejs-backend/shared),
                 LoadCache, orphan GC
  cubeorchestrator/ (extend) QueryOrchestrator + QueryCache on top of the three crates above,
                 reusing query_result_transform.rs for the response shape
```
Trait boundaries: both queue and cache crates depend only on the `cubedriver::Driver` trait
(`query(sql, params, opts) -> Rows`, `stream`, `get_tables_query`, `create_schema_if_not_exists`,
`load_pre_aggregation_into_table`, `download_table/unload`, `upload_table_with_indexes`, `capabilities`,
`now_timestamp`, `table_column_types`). CubeStore access goes through `cubestore-ws-transport::Client`
(`rust/cube/cubestore-ws-transport/src/client.rs:111-150`); note its `query()` currently takes SQL only
(`codec::encode_query`, `codec.rs:16`) — bound parameters and inline tables exist in the FlatBuffers schema
(`cubeshared/src/codegen/http_message_generated.rs:1840-1891`) and must be surfaced before `CACHE SET`/`QUEUE ACK`
can send payloads unescaped; otherwise replicate `formatSql` inlining.

Ordered plan (minimal `/v1/load` without pre-aggregations first):
1. `cubecache::cache_key` — port `getCacheHash`, `queryCacheKey`, `refreshKeyIdentity`; golden tests against the JS hashes (JSON canonicalization is the main risk).
2. `cubecache` MemoryCacheDriver + `CacheEntry {time, result, renewalKey, requestId}`, `decideCacheAction`/`isMemoryEntryUsable` as pure functions with unit tests mirroring `test/unit/QueryCache.test.ts`.
3. `cubequeue` engine + `LocalQueueDriver` (single process): `execute_in_queue`, reconcile, heartbeat, `ContinueWait` error, query stages. Reuse the existing abstract test suites as the acceptance spec.
4. Wire `QueryCache::cached_query_result` branches A/C only (no `backgroundRenew`, no persistent streams) + `QueryOrchestrator::fetch_query` with an empty pre-agg list → this already satisfies `/v1/load` for a cached SQL query; emit the exact result struct from §1 and let `cubeorchestrator::query_result_transform` build the body.
5. Add `CubeStoreCacheDriver` + `CubeStoreQueueDriver` over `cubestore-ws-transport` (after parameter support), enabling multi-node/production parity.
6. Refresh keys: `loadRefreshKeys` with per-key async debounce, `cacheRefreshKeyResult`, local `every` evaluation (`utils.ts:45-53`).
7. `cubepreaggs`: version hashing + table naming + `VersionEntry` parsing (pure, easily golden-tested), then `LoadCache`, then `PreAggregationLoader.loadPreAggregation`, then partitions/time-series, then build strategies and orphan GC.
8. Streaming (`QueryStream`, persistent keys with `@processUid`) and the pre-agg jobs/system endpoints last.
