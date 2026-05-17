use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "lightyear-debug")]
#[command(about = "Inspect Lightyear structured debug JSONL files")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Load JSONL debug files into a DuckDB table.
    Ingest {
        /// DuckDB database path.
        #[arg(long)]
        duckdb: PathBuf,
        /// Destination table name.
        #[arg(long, default_value = "events")]
        table: String,
        /// Print the SQL instead of running DuckDB.
        #[arg(long)]
        print_sql: bool,
        /// JSONL files to ingest. Use `name=path.jsonl` to set source_log.
        files: Vec<String>,
    },
    /// Run a SQL query against the DuckDB database.
    Query {
        /// DuckDB database path.
        #[arg(long)]
        duckdb: PathBuf,
        /// SQL to execute.
        #[arg(long)]
        sql: String,
    },
    /// Print a compact event-count summary.
    Summary {
        /// DuckDB database path.
        #[arg(long)]
        duckdb: PathBuf,
        /// Source table name.
        #[arg(long, default_value = "events")]
        table: String,
    },
    /// Print a component-value time series.
    ComponentSeries {
        /// DuckDB database path.
        #[arg(long)]
        duckdb: PathBuf,
        /// Source table name.
        #[arg(long, default_value = "events")]
        table: String,
        /// Component full or short name to match.
        #[arg(long)]
        component: String,
        /// Optional entity filter.
        #[arg(long)]
        entity: Option<String>,
        /// Maximum rows.
        #[arg(long, default_value_t = 200)]
        limit: u32,
    },
    /// Create a normalized view with source_log, frame_key, and merge_tick_id.
    CreateView {
        /// DuckDB database path.
        #[arg(long)]
        duckdb: PathBuf,
        /// Source table name.
        #[arg(long, default_value = "events")]
        table: String,
        /// View name.
        #[arg(long, default_value = "events_normalized")]
        view: String,
        /// Print the SQL instead of running DuckDB.
        #[arg(long)]
        print_sql: bool,
    },
    /// Print merged events ordered by merge_tick_id/source/frame.
    MergedEvents {
        /// DuckDB database path.
        #[arg(long)]
        duckdb: PathBuf,
        /// Source table name.
        #[arg(long, default_value = "events")]
        table: String,
        /// Minimum merged tick id.
        #[arg(long)]
        from_tick: Option<i64>,
        /// Maximum merged tick id.
        #[arg(long)]
        to_tick: Option<i64>,
        /// Optional source_log filter.
        #[arg(long)]
        source: Vec<String>,
        /// Maximum rows.
        #[arg(long, default_value_t = 500)]
        limit: u32,
        /// Print the SQL instead of running DuckDB.
        #[arg(long)]
        print_sql: bool,
    },
    /// Pivot component values from multiple source logs by merge_tick_id.
    ComponentJoin {
        /// DuckDB database path.
        #[arg(long)]
        duckdb: PathBuf,
        /// Source table name.
        #[arg(long, default_value = "events")]
        table: String,
        /// Component full or short name to match.
        #[arg(long)]
        component: String,
        /// Source logs to pivot, for example `server,client1,client2`.
        #[arg(long, value_delimiter = ',')]
        sources: Vec<String>,
        /// Minimum merged tick id.
        #[arg(long)]
        from_tick: Option<i64>,
        /// Maximum merged tick id.
        #[arg(long)]
        to_tick: Option<i64>,
        /// Maximum rows.
        #[arg(long, default_value_t = 200)]
        limit: u32,
        /// Print the SQL instead of running DuckDB.
        #[arg(long)]
        print_sql: bool,
    },
    /// Pivot input sent/received/buffered rows from multiple source logs by merge_tick_id.
    InputJoin {
        /// DuckDB database path.
        #[arg(long)]
        duckdb: PathBuf,
        /// Source table name.
        #[arg(long, default_value = "events")]
        table: String,
        /// Source logs to pivot, for example `server,client1,client2`.
        #[arg(long, value_delimiter = ',')]
        sources: Vec<String>,
        /// Minimum merged tick id.
        #[arg(long)]
        from_tick: Option<i64>,
        /// Maximum merged tick id.
        #[arg(long)]
        to_tick: Option<i64>,
        /// Maximum rows.
        #[arg(long, default_value_t = 200)]
        limit: u32,
        /// Print the SQL instead of running DuckDB.
        #[arg(long)]
        print_sql: bool,
    },
    /// Show each source log's full debug state for one merged tick.
    TickView {
        /// DuckDB database path.
        #[arg(long)]
        duckdb: PathBuf,
        /// Source table name.
        #[arg(long, default_value = "events")]
        table: String,
        /// Merged tick id to inspect.
        #[arg(long)]
        tick: i64,
        /// Source logs to include, for example `server,client1,client2`.
        #[arg(long, value_delimiter = ',')]
        sources: Vec<String>,
        /// Optional categories to include, for example `input,component,prediction`.
        #[arg(long, value_delimiter = ',')]
        categories: Vec<String>,
        /// Maximum events retained in each source's event list.
        #[arg(long, default_value_t = 200)]
        limit: u32,
        /// Print the SQL instead of running DuckDB.
        #[arg(long)]
        print_sql: bool,
    },
    /// Show focused input and component state for one tick.
    TickState {
        /// DuckDB database path.
        #[arg(long)]
        duckdb: PathBuf,
        /// Source table name.
        #[arg(long, default_value = "events")]
        table: String,
        /// Tick to inspect.
        #[arg(long)]
        tick: i64,
        /// Source logs to include, for example `server,client1,client2`.
        #[arg(long, value_delimiter = ',')]
        sources: Vec<String>,
        /// Component full or short names to include. Empty means all components.
        #[arg(long, value_delimiter = ',')]
        components: Vec<String>,
        /// Maximum characters retained from each full input buffer.
        #[arg(long, default_value_t = 240)]
        buffer_chars: u32,
        /// Print the SQL instead of running DuckDB.
        #[arg(long)]
        print_sql: bool,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Commands::Ingest {
            duckdb,
            table,
            print_sql,
            files,
        } => {
            if files.is_empty() {
                return Err("ingest requires at least one JSONL file".to_string());
            }
            let sql = ingest_sql(&table, &files)?;
            if print_sql {
                println!("{sql}");
                return Ok(());
            }
            run_duckdb(&duckdb, &sql)
        }
        Commands::Query { duckdb, sql } => run_duckdb(&duckdb, &sql),
        Commands::Summary { duckdb, table } => {
            let table = sql_ident(&table)?;
            run_duckdb(
                &duckdb,
                &format!(
                    "SELECT target, kind, count(*) AS rows \
                     FROM {table} \
                     GROUP BY target, kind \
                     ORDER BY rows DESC, target, kind;"
                ),
            )
        }
        Commands::ComponentSeries {
            duckdb,
            table,
            component,
            entity,
            limit,
        } => {
            let table = sql_ident(&table)?;
            let component = sql_string(&component);
            let entity_filter = entity
                .as_ref()
                .map(|entity| format!(" AND entity = {}", sql_string(entity)))
                .unwrap_or_default();
            run_duckdb(
                &duckdb,
                &format!(
                    "SELECT timestamp, frame_id, tick, sample_point, entity, component, value \
                     FROM {table} \
                     WHERE category = 'component' \
                       AND (component = {component} OR regexp_extract(component, '[^:<>]+(?:<.*>)?$') = {component})\
                       {entity_filter} \
                     ORDER BY timestamp, frame_id \
                     LIMIT {limit};"
                ),
            )
        }
        Commands::CreateView {
            duckdb,
            table,
            view,
            print_sql,
        } => {
            let sql = create_view_sql(&table, &view)?;
            if print_sql {
                println!("{sql}");
                return Ok(());
            }
            run_duckdb(&duckdb, &sql)
        }
        Commands::MergedEvents {
            duckdb,
            table,
            from_tick,
            to_tick,
            source,
            limit,
            print_sql,
        } => {
            let sql = merged_events_sql(&table, from_tick, to_tick, &source, limit)?;
            if print_sql {
                println!("{sql}");
                return Ok(());
            }
            run_duckdb(&duckdb, &sql)
        }
        Commands::ComponentJoin {
            duckdb,
            table,
            component,
            sources,
            from_tick,
            to_tick,
            limit,
            print_sql,
        } => {
            let sql = component_join_sql(&table, &component, &sources, from_tick, to_tick, limit)?;
            if print_sql {
                println!("{sql}");
                return Ok(());
            }
            run_duckdb(&duckdb, &sql)
        }
        Commands::InputJoin {
            duckdb,
            table,
            sources,
            from_tick,
            to_tick,
            limit,
            print_sql,
        } => {
            let sql = input_join_sql(&table, &sources, from_tick, to_tick, limit)?;
            if print_sql {
                println!("{sql}");
                return Ok(());
            }
            run_duckdb(&duckdb, &sql)
        }
        Commands::TickView {
            duckdb,
            table,
            tick,
            sources,
            categories,
            limit,
            print_sql,
        } => {
            let sql = tick_view_sql(&table, tick, &sources, &categories, limit)?;
            if print_sql {
                println!("{sql}");
                return Ok(());
            }
            run_duckdb(&duckdb, &sql)
        }
        Commands::TickState {
            duckdb,
            table,
            tick,
            sources,
            components,
            buffer_chars,
            print_sql,
        } => {
            let sql = tick_state_sql(&table, tick, &sources, &components, buffer_chars)?;
            if print_sql {
                println!("{sql}");
                return Ok(());
            }
            run_duckdb(&duckdb, &sql)
        }
    }
}

fn ingest_sql(table: &str, files: &[String]) -> Result<String, String> {
    let table = sql_ident(table)?;
    let columns = read_json_columns_sql();
    let files = files
        .iter()
        .map(|value| {
            let input = LogInput::parse(value)?;
            if !input.path.exists() {
                return Err(format!(
                    "debug file does not exist: {}",
                    input.path.display()
                ));
            }
            Ok(format!(
                "SELECT *, {} AS source_log, {} AS source_path \
                 FROM read_json_auto({}, \
                   format = 'newline_delimited', \
                   union_by_name = true, \
                   ignore_errors = true, \
                   columns = {columns})",
                sql_string(&input.name),
                sql_os_string(&input.path),
                sql_os_string(&input.path),
            ))
        })
        .collect::<Result<Vec<_>, _>>()?
        .join(" UNION ALL BY NAME ");
    Ok(format!(
        "CREATE OR REPLACE TABLE {table} AS {files};\n{}",
        standard_columns_sql(&table)
    ))
}

#[derive(Debug)]
struct LogInput {
    name: String,
    path: PathBuf,
}

impl LogInput {
    fn parse(value: &str) -> Result<Self, String> {
        let (name, path) = match value.split_once('=') {
            Some((name, path)) if !name.is_empty() && !path.is_empty() => {
                (name.to_string(), PathBuf::from(path))
            }
            _ => {
                let path = PathBuf::from(value);
                let name = path
                    .file_stem()
                    .and_then(OsStr::to_str)
                    .filter(|name| !name.is_empty())
                    .unwrap_or("log")
                    .to_string();
                (name, path)
            }
        };
        Ok(Self { name, path })
    }
}

fn debug_columns() -> &'static [(&'static str, &'static str)] {
    &[
        ("timestamp", "UBIGINT"),
        ("frame_id", "UBIGINT"),
        ("tick_id", "BIGINT"),
        ("tick", "VARCHAR"),
        ("local_tick", "BIGINT"),
        ("input_tick", "BIGINT"),
        ("server_tick", "BIGINT"),
        ("remote_tick", "BIGINT"),
        ("end_tick", "BIGINT"),
        ("confirmed_tick", "BIGINT"),
        ("interpolation_tick", "BIGINT"),
        ("rollback_tick", "BIGINT"),
        ("target", "VARCHAR"),
        ("category", "VARCHAR"),
        ("kind", "VARCHAR"),
        ("sample_point", "VARCHAR"),
        ("schedule", "VARCHAR"),
        ("system", "VARCHAR"),
        ("system_set", "VARCHAR"),
        ("role", "VARCHAR"),
        ("run_id", "VARCHAR"),
        ("app_id", "VARCHAR"),
        ("client_id", "VARCHAR"),
        ("local_id", "VARCHAR"),
        ("remote_id", "VARCHAR"),
        ("remote_peer", "VARCHAR"),
        ("entity", "VARCHAR"),
        ("source_entity", "VARCHAR"),
        ("remote_entity", "VARCHAR"),
        ("link_entity", "VARCHAR"),
        ("component", "VARCHAR"),
        ("action", "VARCHAR"),
        ("direction", "VARCHAR"),
        ("channel", "VARCHAR"),
        ("channel_id", "VARCHAR"),
        ("message_id", "VARCHAR"),
        ("message_name", "VARCHAR"),
        ("message_net_id", "VARCHAR"),
        ("num_messages", "BIGINT"),
        ("packet_id", "VARCHAR"),
        ("priority", "VARCHAR"),
        ("buffer_len", "BIGINT"),
        ("bytes", "BIGINT"),
        ("send_bytes", "BIGINT"),
        ("rtt_ms", "DOUBLE"),
        ("jitter_ms", "DOUBLE"),
        ("packet_loss", "DOUBLE"),
        ("value", "JSON"),
        ("fields", "JSON"),
    ]
}

fn read_json_columns_sql() -> String {
    let columns = debug_columns()
        .iter()
        .map(|(column, ty)| format!("{column}: {}", sql_string(*ty)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{columns}}}")
}

fn standard_columns_sql(table: &str) -> String {
    debug_columns()
        .iter()
        .copied()
        .chain([
            ("source_log", "VARCHAR DEFAULT 'log'"),
            ("source_path", "VARCHAR"),
        ])
        .map(|(column, ty)| {
            format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS \"{column}\" {ty};")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn create_view_sql(table: &str, view: &str) -> Result<String, String> {
    let view = sql_ident(view)?;
    Ok(format!(
        "CREATE OR REPLACE VIEW {view} AS {};",
        normalized_select_sql(table)?
    ))
}

fn merged_events_sql(
    table: &str,
    from_tick: Option<i64>,
    to_tick: Option<i64>,
    sources: &[String],
    limit: u32,
) -> Result<String, String> {
    let filters = tick_and_source_filters(from_tick, to_tick, sources);
    Ok(format!(
        "WITH normalized AS ({}) \
         SELECT source_log, merge_tick_id AS tick_id, frame_key, frame_id, timestamp, \
                category, kind, sample_point, schedule, entity, component, value, fields \
         FROM normalized \
         {filters} \
         ORDER BY merge_tick_id NULLS LAST, source_log, frame_id, timestamp \
         LIMIT {limit};",
        normalized_select_sql(table)?
    ))
}

fn component_join_sql(
    table: &str,
    component: &str,
    sources: &[String],
    from_tick: Option<i64>,
    to_tick: Option<i64>,
    limit: u32,
) -> Result<String, String> {
    let component = sql_string(component);
    let filters = tick_and_source_filters(from_tick, to_tick, sources);
    let pivot_columns = if sources.is_empty() {
        "list(struct_pack(source_log := source_log, frame_key := frame_key, entity := entity, value := value)) AS values_by_source".to_string()
    } else {
        sources
            .iter()
            .map(|source| {
                let ident = sql_ident(source)?;
                Ok(format!(
                    "list(struct_pack(frame_key := frame_key, entity := entity, value := value)) \
                     FILTER (WHERE source_log = {}) AS {ident}",
                    sql_string(source),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?
            .join(", ")
    };
    Ok(format!(
        "WITH normalized AS ({}), \
              components AS ( \
                SELECT * FROM normalized \
                WHERE category = 'component' \
                  AND merge_tick_id IS NOT NULL \
                  AND (component = {component} OR regexp_extract(component, '[^:<>]+(?:<.*>)?$') = {component}) \
              ) \
         SELECT merge_tick_id AS tick_id, component, {pivot_columns} \
         FROM components \
         {filters} \
         GROUP BY merge_tick_id, component \
         ORDER BY merge_tick_id \
         LIMIT {limit};",
        normalized_select_sql(table)?
    ))
}

fn input_join_sql(
    table: &str,
    sources: &[String],
    from_tick: Option<i64>,
    to_tick: Option<i64>,
    limit: u32,
) -> Result<String, String> {
    let filters = tick_and_source_filters_for("input_join_tick", from_tick, to_tick, sources);
    let pivot_columns = if sources.is_empty() {
        "list(input_row ORDER BY source_log, frame_id, timestamp) AS inputs_by_source".to_string()
    } else {
        sources
            .iter()
            .map(|source| {
                let ident = sql_ident(source)?;
                Ok(format!(
                    "list(input_row ORDER BY frame_id, timestamp) \
                     FILTER (WHERE source_log = {}) AS {ident}",
                    sql_string(source),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?
            .join(", ")
    };
    Ok(format!(
        "WITH normalized AS ({}), \
              inputs AS ( \
                SELECT *, \
                       COALESCE( \
                         try_cast(end_tick AS BIGINT), \
                         try_cast(input_tick AS BIGINT), \
                         try_cast(local_tick AS BIGINT), \
                         merge_tick_id \
                       ) AS input_join_tick, \
                       CASE \
                         WHEN strpos(lower(kind), 'recv') > 0 OR strpos(lower(kind), 'receive') > 0 THEN 'received' \
                         WHEN strpos(lower(kind), 'send') > 0 OR strpos(lower(kind), 'prepare') > 0 THEN 'sent' \
                         WHEN strpos(lower(kind), 'buffer') > 0 OR strpos(lower(kind), 'action_state') > 0 THEN 'buffered' \
                         ELSE 'other' \
                       END AS input_flow \
                FROM normalized \
                WHERE category = 'input' \
                  AND merge_tick_id IS NOT NULL \
              ), \
              packed AS ( \
                SELECT input_join_tick, source_log, frame_id, timestamp, \
                       struct_pack( \
                         source_log := source_log, \
                         frame_key := frame_key, \
                         flow := input_flow, \
                         kind := kind, \
                         sample_point := sample_point, \
                         entity := entity, \
                         local_tick := local_tick, \
                         input_tick := input_tick, \
                         end_tick := end_tick, \
                         value := value, \
                         fields := fields \
                       ) AS input_row \
                FROM inputs \
              ) \
         SELECT input_join_tick AS tick_id, {pivot_columns} \
         FROM packed \
         {filters} \
         GROUP BY input_join_tick \
         ORDER BY input_join_tick \
         LIMIT {limit};",
        normalized_select_sql(table)?
    ))
}

fn tick_view_sql(
    table: &str,
    tick: i64,
    sources: &[String],
    categories: &[String],
    limit: u32,
) -> Result<String, String> {
    let mut filters = vec![tick_match_filter(tick)];
    if !sources.is_empty() {
        let sources = sources
            .iter()
            .map(sql_string)
            .collect::<Vec<_>>()
            .join(", ");
        filters.push(format!("source_log IN ({sources})"));
    }
    if !categories.is_empty() {
        let categories = categories
            .iter()
            .map(sql_string)
            .collect::<Vec<_>>()
            .join(", ");
        filters.push(format!("category IN ({categories})"));
    }
    let filters = filters.join(" AND ");
    Ok(format!(
        "WITH normalized AS ({}), \
              tick_rows AS ( \
                SELECT * FROM normalized \
                WHERE {filters} \
              ), \
              numbered AS ( \
                SELECT *, \
                       row_number() OVER (PARTITION BY source_log ORDER BY frame_id, timestamp) AS event_rank \
                FROM tick_rows \
              ) \
         SELECT source_log, \
                min(timestamp) AS first_timestamp, \
                count(*) AS rows, \
                count(*) FILTER (WHERE category = 'input') AS input_rows, \
                count(*) FILTER (WHERE category = 'component') AS component_rows, \
                count(*) FILTER (WHERE category = 'prediction') AS prediction_rows, \
                count(*) FILTER (WHERE category = 'timeline') AS timeline_rows, \
                count(*) FILTER (WHERE strpos(lower(kind), 'rollback') > 0) AS rollback_rows, \
                list(struct_pack( \
                  frame_key := frame_key, \
                  frame_id := frame_id, \
                  event_timestamp := timestamp, \
                  category := category, \
                  kind := kind, \
                  sample_point := sample_point, \
                  schedule := schedule, \
                  entity := entity, \
                  component := component, \
                  value := value, \
                  fields := fields \
                ) ORDER BY frame_id, timestamp) FILTER (WHERE event_rank <= {limit}) AS events \
         FROM numbered \
         GROUP BY source_log \
         ORDER BY source_log;",
        normalized_select_sql(table)?
    ))
}

fn tick_state_sql(
    table: &str,
    tick: i64,
    sources: &[String],
    components: &[String],
    buffer_chars: u32,
) -> Result<String, String> {
    let source_filter = source_filter_sql(sources);
    let component_filter = component_filter_sql(components)?;
    let tick_literal = tick;
    let source_entity_map_columns = if sources.is_empty() {
        String::new()
    } else {
        sources
            .iter()
            .map(|source| {
                let ident = sql_ident(&format!("{source}_entities"))?;
                Ok(format!(
                    "string_agg(DISTINCT entity, ', ') FILTER (WHERE source_log = {}) AS {ident}",
                    sql_string(source),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?
            .join(", ")
    };
    let entity_map_columns = if source_entity_map_columns.is_empty() {
        "string_agg(DISTINCT source_log || ':' || entity, ', ') AS entities_by_source".to_string()
    } else {
        format!(
            "string_agg(DISTINCT source_log || ':' || entity, ', ') AS entities_by_source, {source_entity_map_columns}"
        )
    };
    Ok(format!(
        "WITH normalized AS ({}), \
              entity_labels AS ( \
                SELECT source_log, merge_tick_id AS tick_id, entity, \
                       string_agg(DISTINCT \
                         regexp_replace( \
                           COALESCE(json_extract_string(value, '$'), CAST(value AS VARCHAR), json_extract_string(fields, '$.position'), CAST(fields AS VARCHAR)), \
                           '\"', \
                           '' \
                         ), \
                         ', ' \
                       ) AS entity_label \
                FROM normalized \
                WHERE category = 'component' \
                  AND merge_tick_id = {tick_literal} \
                  {source_filter} \
                  AND (component LIKE '%::PlayerId' OR component LIKE '%::Name' OR component = 'PlayerId' OR component = 'Name') \
                GROUP BY source_log, merge_tick_id, entity \
              ) \
         SELECT 'entity_map' AS row_type, \
                tick_id, \
                entity_label, \
                {entity_map_columns} \
         FROM entity_labels \
         GROUP BY tick_id, entity_label \
         ORDER BY tick_id, entity_label; \
         WITH normalized AS ({}), \
              input_rows AS ( \
                SELECT DISTINCT \
                       'input' AS row_type, \
                       source_log, \
                       COALESCE(try_cast(end_tick AS BIGINT), try_cast(input_tick AS BIGINT), try_cast(local_tick AS BIGINT), merge_tick_id) AS input_join_tick, \
                       try_cast(local_tick AS BIGINT) AS app_local_tick, \
                       try_cast(input_tick AS BIGINT) AS input_timeline_tick, \
                       try_cast(end_tick AS BIGINT) AS input_message_end_tick, \
                       kind, sample_point, schedule, entity AS action_entity, \
                       COALESCE( \
                         json_extract_string(fields, '$.target'), \
                         regexp_extract(json_extract_string(fields, '$.message'), 'target: ([^,}}]+)', 1) \
                       ) AS input_target, \
                       try_cast(json_extract_string(fields, '$.num_targets') AS BIGINT) AS num_targets, \
                       regexp_extract( \
                         COALESCE( \
                           json_extract_string(fields, '$.snapshot'), \
                           json_extract_string(fields, '$.states'), \
                           json_extract_string(fields, '$.input_buffer'), \
                           json_extract_string(fields, '$.message'), \
                           CAST(fields AS VARCHAR) \
                         ), \
                         'Axis2D[(]Vec2[(](-?[0-9.]+),[[:space:]]*(-?[0-9.]+)[)][)]', \
                         0 \
                       ) AS action_axis, \
                       json_extract_string(fields, '$.snapshot') AS action_state, \
                       json_extract_string(fields, '$.states') AS sent_states, \
                       regexp_extract( \
                         json_extract_string(fields, '$.input_buffer'), \
                         'Tick\\\\({tick_literal}\\\\): ([^\\\\n]+)', \
                         1 \
                       ) AS input_buffer_tick_value, \
                       left(json_extract_string(fields, '$.input_buffer'), {buffer_chars}) AS input_buffer_excerpt \
                FROM normalized \
                WHERE category = 'input' \
                  AND COALESCE(try_cast(end_tick AS BIGINT), try_cast(input_tick AS BIGINT), try_cast(local_tick AS BIGINT), merge_tick_id) = {tick_literal} \
                  {source_filter} \
                  AND kind IN ( \
                    'buffer_action_state', \
                    'get_action_state', \
                    'get_delayed_action_state', \
                    'prepare_input_message_target', \
                    'prepare_input_message_finish', \
                    'server_input_message_recv', \
                    'server_input_buffer_update', \
                    'server_update_action_state' \
                  ) \
              ) \
         SELECT * FROM input_rows \
         ORDER BY source_log, action_entity NULLS LAST, kind, sample_point; \
         WITH normalized AS ({}), \
              entity_labels AS ( \
                SELECT source_log, merge_tick_id AS tick_id, entity, \
                       string_agg(DISTINCT \
                         regexp_replace( \
                           COALESCE(json_extract_string(value, '$'), CAST(value AS VARCHAR), json_extract_string(fields, '$.position'), CAST(fields AS VARCHAR)), \
                           '\"', \
                           '' \
                         ), \
                         ', ' \
                       ) AS entity_label \
                FROM normalized \
                WHERE category = 'component' \
                  AND merge_tick_id = {tick_literal} \
                  {source_filter} \
                  AND (component LIKE '%::PlayerId' OR component LIKE '%::Name' OR component = 'PlayerId' OR component = 'Name') \
                GROUP BY source_log, merge_tick_id, entity \
              ), \
              component_raw AS ( \
                SELECT source_log, merge_tick_id AS tick_id, kind, sample_point, schedule, entity, component, \
                       COALESCE(json_extract_string(value, '$'), CAST(value AS VARCHAR), json_extract_string(fields, '$.position'), CAST(fields AS VARCHAR)) AS component_text, \
                       try_cast(regexp_extract( \
                         COALESCE(json_extract_string(value, '$'), CAST(value AS VARCHAR), json_extract_string(fields, '$.position'), CAST(fields AS VARCHAR)), \
                         'Vec2[(](-?[0-9.]+),[[:space:]]*(-?[0-9.]+)[)]', \
                         1 \
                       ) AS DOUBLE) AS x, \
                       try_cast(regexp_extract( \
                         COALESCE(json_extract_string(value, '$'), CAST(value AS VARCHAR), json_extract_string(fields, '$.position'), CAST(fields AS VARCHAR)), \
                         'Vec2[(](-?[0-9.]+),[[:space:]]*(-?[0-9.]+)[)]', \
                         2 \
                       ) AS DOUBLE) AS y \
                FROM normalized \
                WHERE category = 'component' \
                  AND merge_tick_id = {tick_literal} \
                  {source_filter} \
                  {component_filter} \
              ), \
              component_rows AS ( \
                SELECT 'component' AS row_type, \
                       source_log, \
                       tick_id, \
                       kind, sample_point, schedule, entity, component, \
                       max(entity_label) AS entity_label, \
                       count(*) AS rows, \
                       CAST(round(min(x), 4) AS VARCHAR) AS min_x, \
                       CAST(round(max(x), 4) AS VARCHAR) AS max_x, \
                       CAST(round(min(y), 4) AS VARCHAR) AS min_y, \
                       CAST(round(max(y), 4) AS VARCHAR) AS max_y \
                FROM component_raw \
                LEFT JOIN entity_labels USING (source_log, tick_id, entity) \
                GROUP BY source_log, tick_id, kind, sample_point, schedule, entity, component \
              ) \
        SELECT * FROM component_rows \
        ORDER BY source_log, entity, component, kind, sample_point;",
        normalized_select_sql(table)?,
        normalized_select_sql(table)?,
        normalized_select_sql(table)?,
        entity_map_columns = entity_map_columns,
    ))
}

fn tick_match_filter(tick: i64) -> String {
    let comparisons = [
        "tick_id",
        "local_tick",
        "input_tick",
        "server_tick",
        "remote_tick",
        "end_tick",
        "confirmed_tick",
        "interpolation_tick",
        "merge_tick_id",
    ]
    .into_iter()
    .map(|column| format!("try_cast({column} AS BIGINT) = {tick}"))
    .chain([format!(
        "try_cast(regexp_extract(CAST(tick AS VARCHAR), '([0-9]+)', 1) AS BIGINT) = {tick}"
    )]);
    format!("({})", comparisons.collect::<Vec<_>>().join(" OR "))
}

fn source_filter_sql(sources: &[String]) -> String {
    if sources.is_empty() {
        String::new()
    } else {
        let sources = sources
            .iter()
            .map(sql_string)
            .collect::<Vec<_>>()
            .join(", ");
        format!("AND source_log IN ({sources})")
    }
}

fn component_filter_sql(components: &[String]) -> Result<String, String> {
    if components.is_empty() {
        return Ok(String::new());
    }
    let components = components
        .iter()
        .map(sql_string)
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "AND (component IN ({components}) OR regexp_extract(component, '[^:<>]+(?:<.*>)?$') IN ({components}))"
    ))
}

fn normalized_select_sql(table: &str) -> Result<String, String> {
    let table = sql_ident(table)?;
    Ok(format!(
        "SELECT *, \
                COALESCE( \
                  try_cast(tick_id AS BIGINT), \
                  try_cast(local_tick AS BIGINT), \
                  try_cast(input_tick AS BIGINT), \
                  try_cast(server_tick AS BIGINT), \
                  try_cast(remote_tick AS BIGINT), \
                  try_cast(end_tick AS BIGINT), \
                  try_cast(confirmed_tick AS BIGINT), \
                  try_cast(interpolation_tick AS BIGINT), \
                  try_cast(regexp_extract(CAST(tick AS VARCHAR), '([0-9]+)', 1) AS BIGINT) \
                ) AS merge_tick_id, \
                source_log || ':' || CAST(frame_id AS VARCHAR) AS frame_key \
         FROM {table}"
    ))
}

fn tick_and_source_filters(
    from_tick: Option<i64>,
    to_tick: Option<i64>,
    sources: &[String],
) -> String {
    tick_and_source_filters_for("merge_tick_id", from_tick, to_tick, sources)
}

fn tick_and_source_filters_for(
    tick_column: &str,
    from_tick: Option<i64>,
    to_tick: Option<i64>,
    sources: &[String],
) -> String {
    let mut filters = Vec::new();
    if let Some(from_tick) = from_tick {
        filters.push(format!("{tick_column} >= {from_tick}"));
    }
    if let Some(to_tick) = to_tick {
        filters.push(format!("{tick_column} <= {to_tick}"));
    }
    if !sources.is_empty() {
        let sources = sources
            .iter()
            .map(sql_string)
            .collect::<Vec<_>>()
            .join(", ");
        filters.push(format!("source_log IN ({sources})"));
    }
    if filters.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filters.join(" AND "))
    }
}

fn run_duckdb(database: &Path, sql: &str) -> Result<(), String> {
    let status = Command::new("duckdb")
        .arg(database)
        .arg("-c")
        .arg(sql)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| {
            format!(
                "failed to run duckdb: {error}\n\
                 install the DuckDB CLI or rerun `ingest` with `--print-sql`"
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("duckdb exited with status {status}"))
    }
}

fn sql_ident(value: &str) -> Result<String, String> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(format!("invalid SQL identifier: {value:?}"));
    }
    Ok(format!("\"{value}\""))
}

fn sql_string(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    format!("'{}'", value.replace('\'', "''"))
}

fn sql_os_string(value: impl AsRef<OsStr>) -> String {
    let value = value.as_ref().to_string_lossy();
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_jsonl(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("lightyear-debug-{name}-{unique}.jsonl"));
        fs::write(&path, "{}\n").unwrap();
        path
    }

    #[test]
    fn ingest_sql_tags_each_file_with_source_log() {
        let server = temp_jsonl("server");
        let client = temp_jsonl("client");
        let inputs = vec![
            format!("server={}", server.display()),
            format!("client1={}", client.display()),
        ];

        let sql = ingest_sql("events", &inputs).unwrap();

        assert!(sql.contains("'server' AS source_log"));
        assert!(sql.contains("'client1' AS source_log"));
        assert!(sql.contains("UNION ALL BY NAME"));
        assert!(sql.contains("ADD COLUMN IF NOT EXISTS \"tick_id\""));
        assert!(sql.contains("ADD COLUMN IF NOT EXISTS \"source_log\""));

        let _ = fs::remove_file(server);
        let _ = fs::remove_file(client);
    }

    #[test]
    fn merged_events_sql_uses_source_qualified_frame_and_tick_filters() {
        let sql = merged_events_sql(
            "events",
            Some(10),
            Some(20),
            &["server".to_string(), "client1".to_string()],
            50,
        )
        .unwrap();

        assert!(sql.contains("merge_tick_id AS tick_id"));
        assert!(sql.contains("source_log || ':' || CAST(frame_id AS VARCHAR) AS frame_key"));
        assert!(sql.contains("merge_tick_id >= 10"));
        assert!(sql.contains("merge_tick_id <= 20"));
        assert!(sql.contains("source_log IN ('server', 'client1')"));
    }

    #[test]
    fn component_join_sql_pivots_requested_sources() {
        let sql = component_join_sql(
            "events",
            "Position",
            &["server".to_string(), "client1".to_string()],
            None,
            None,
            25,
        )
        .unwrap();

        assert!(sql.contains("merge_tick_id AS tick_id"));
        assert!(sql.contains("FILTER (WHERE source_log = 'server') AS \"server\""));
        assert!(sql.contains("FILTER (WHERE source_log = 'client1') AS \"client1\""));
        assert!(sql.contains("component = 'Position'"));
        assert!(sql.contains("source_log IN ('server', 'client1')"));
    }

    #[test]
    fn input_join_sql_pivots_sources_and_classifies_flows() {
        let sql = input_join_sql(
            "events",
            &["server".to_string(), "client1".to_string()],
            Some(5),
            Some(7),
            10,
        )
        .unwrap();

        assert!(sql.contains("category = 'input'"));
        assert!(sql.contains("flow := input_flow"));
        assert!(sql.contains("FILTER (WHERE source_log = 'server') AS \"server\""));
        assert!(sql.contains("FILTER (WHERE source_log = 'client1') AS \"client1\""));
        assert!(sql.contains("input_join_tick >= 5"));
        assert!(sql.contains("input_join_tick <= 7"));
    }

    #[test]
    fn tick_view_sql_groups_per_source_with_category_filters() {
        let sql = tick_view_sql(
            "events",
            42,
            &["server".to_string(), "client1".to_string()],
            &["input".to_string(), "component".to_string()],
            25,
        )
        .unwrap();

        assert!(sql.contains("try_cast(merge_tick_id AS BIGINT) = 42"));
        assert!(sql.contains("source_log IN ('server', 'client1')"));
        assert!(sql.contains("category IN ('input', 'component')"));
        assert!(sql.contains("GROUP BY source_log"));
        assert!(sql.contains("timeline_rows"));
        assert!(sql.contains("rollback_rows"));
        assert!(sql.contains("event_rank <= 25"));
    }

    #[test]
    fn tick_state_sql_extracts_input_and_component_state() {
        let sql = tick_state_sql(
            "events",
            148,
            &["server".to_string(), "client1".to_string()],
            &["PlayerPosition".to_string()],
            128,
        )
        .unwrap();

        assert!(sql.contains("'input' AS row_type"));
        assert!(sql.contains("'component' AS row_type"));
        assert!(sql.contains("'entity_map' AS row_type"));
        assert!(sql.contains("AS \"server_entities\""));
        assert!(sql.contains("AS \"client1_entities\""));
        assert!(sql.contains("entities_by_source"));
        assert!(sql.contains("Tick\\\\(148\\\\):"));
        assert!(sql.contains("input_buffer_tick_value"));
        assert!(sql.contains("action_state"));
        assert!(sql.contains("Axis2D[(]Vec2[(]"));
        assert!(sql.contains("input_join_tick"));
        assert!(sql.contains("app_local_tick"));
        assert!(sql.contains("input_timeline_tick"));
        assert!(sql.contains("input_message_end_tick"));
        assert!(sql.contains("input_target"));
        assert!(sql.contains("entity_label"));
        assert!(sql.contains("round(min(x), 4)"));
        assert!(sql.contains("Vec2[(](-?[0-9.]+)"));
        assert!(sql.contains("SELECT * FROM input_rows"));
        assert!(sql.contains("SELECT * FROM component_rows"));
        assert!(sql.contains("json_extract_string(value, '$')"));
        assert!(sql.contains("source_log IN ('server', 'client1')"));
        assert!(sql.contains("component IN ('PlayerPosition')"));
        assert!(sql.contains("left(json_extract_string(fields, '$.input_buffer'), 128)"));
    }
}
