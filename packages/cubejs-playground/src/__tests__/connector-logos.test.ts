import { describe, expect, it } from 'vitest';

import { CONNECTOR_LOGOS } from '../shared/connector-logos';
import envVarsDbMap from '../shared/env-vars-db-map';

// Every connector the server builds (cubedriver's IMPLEMENTED_DB_TYPES).
const CONNECTORS = [
  'postgres', 'mysql', 'clickhouse', 'bigquery', 'snowflake', 'mssql', 'redshift', 'druid', 'firebolt',
  'cubestore', 'crate', 'materialize', 'questdb', 'vertica', 'mongobi', 'prestodb', 'trino', 'pinot',
  'athena', 'mysqlauroraserverless', 'dremio', 'ksql', 'databricks-jdbc', 'hive', 'oracle', 'sqlite', 'duckdb',
];

describe('connector logos', () => {
  it('every connector has a logo', () => {
    const mapped = new Map<string, string | undefined>(
      envVarsDbMap.flatMap((g: any) => g.databases.map((db: any) => [db.driver, db.logo]))
    );
    for (const driver of CONNECTORS) {
      const logo = mapped.get(driver) ?? CONNECTOR_LOGOS[driver]?.logo;
      expect(logo, driver).toBeTruthy();
    }
  });
});
