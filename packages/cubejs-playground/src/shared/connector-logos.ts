import logoCubestore from '../img/db/cubestore.svg';
import logoDatabricks from '../img/db/databricks.svg';
import logoDremio from '../img/db/dremio.svg';
import logoDuckdb from '../img/db/duckdb.svg';
import logoKsqldb from '../img/db/ksqldb.svg';
import logoMysql from '../img/db/mysql.svg';
import logoPinot from '../img/db/pinot.svg';
import logoSqlite from '../img/db/sqlite.svg';
import logoTrino from '../img/db/trino.svg';
import logoVertica from '../img/db/vertica.svg';

// The name and logo of each connector the wizard's map (env-vars-db-map)
// does not list, by its driver.
export const CONNECTOR_LOGOS: Record<string, { title: string; logo: string }> = {
  cubestore: { title: 'Cube Store', logo: logoCubestore },
  'databricks-jdbc': { title: 'Databricks', logo: logoDatabricks },
  dremio: { title: 'Dremio', logo: logoDremio },
  duckdb: { title: 'DuckDB', logo: logoDuckdb },
  ksql: { title: 'ksqlDB', logo: logoKsqldb },
  mysqlauroraserverless: { title: 'MySQL Aurora Serverless', logo: logoMysql },
  pinot: { title: 'Apache Pinot', logo: logoPinot },
  sqlite: { title: 'SQLite', logo: logoSqlite },
  trino: { title: 'Trino', logo: logoTrino },
  vertica: { title: 'Vertica', logo: logoVertica },
};
