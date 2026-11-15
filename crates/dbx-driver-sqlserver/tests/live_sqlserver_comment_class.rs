//! Opt-in live regression for column comments in metadata and autocomplete.
//! An index `MS_Description` is stored with `class = 7` and `minor_id = index_id`,
//! so a lookup that does not filter `class = 1` matches the column whose
//! `column_id` equals that `index_id`.
use dbx_driver_sqlserver as native;
use native::types::{CompletionAssistantMatchMode, CompletionAssistantObjectKind, CompletionAssistantRequest};
use std::time::Duration;

#[tokio::test]
#[ignore = "requires DBX_LIVE_SQLSERVER_HOST/PORT/USER/PASSWORD pointing at a writable SQL Server"]
async fn live_sqlserver_index_comments_do_not_leak_into_column_or_table_comments() {
    let host = std::env::var("DBX_LIVE_SQLSERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = std::env::var("DBX_LIVE_SQLSERVER_PORT").ok().and_then(|value| value.parse().ok()).unwrap_or(1433);
    let username = std::env::var("DBX_LIVE_SQLSERVER_USER").unwrap_or_else(|_| "sa".to_string());
    let password = std::env::var("DBX_LIVE_SQLSERVER_PASSWORD").expect("DBX_LIVE_SQLSERVER_PASSWORD");
    let mut client = native::connect(&host, port, &username, &password, Some("tempdb"), None, Duration::from_secs(15))
        .await
        .expect("connect to SQL Server");

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let table = format!("dbx_eval_comment_class_{suffix}");
    let setup = format!(
        "CREATE TABLE dbo.[{table}] (id int NOT NULL CONSTRAINT [pk_{table}] PRIMARY KEY, name nvarchar(50), code int); \
         CREATE INDEX [ix_{table}_code] ON dbo.[{table}](code); \
         EXEC sys.sp_addextendedproperty N'MS_Description', N'pk index comment', N'SCHEMA', N'dbo', N'TABLE', N'{table}', N'INDEX', N'pk_{table}'; \
         EXEC sys.sp_addextendedproperty N'MS_Description', N'code index comment', N'SCHEMA', N'dbo', N'TABLE', N'{table}', N'INDEX', N'ix_{table}_code'; \
         EXEC sys.sp_addextendedproperty N'MS_Description', N'用户名称', N'SCHEMA', N'dbo', N'TABLE', N'{table}', N'COLUMN', N'name'; \
         EXEC sys.sp_addextendedproperty N'OtherProperty', N'not a column comment', N'SCHEMA', N'dbo', N'TABLE', N'{table}', N'COLUMN', N'code'; \
         EXEC sys.sp_addextendedproperty N'MS_Description', N'table comment', N'SCHEMA', N'dbo', N'TABLE', N'{table}';"
    );
    native::execute_simple_batch_with_max_rows(&mut client, &setup, Some(1)).await.expect("create test table");

    let columns = native::get_columns(&mut client, "dbo", &table).await;
    let table_comment = native::get_table_comment(&mut client, "dbo", &table).await;
    let tables = native::list_tables(&mut client, "dbo", Some(&table), None, None).await;
    let objects = native::list_objects(&mut client, "dbo").await;
    let request = CompletionAssistantRequest {
        connection_id: "live-sqlserver".to_string(),
        database: "tempdb".to_string(),
        schema: Some("dbo".to_string()),
        object_kinds: vec![CompletionAssistantObjectKind::Column],
        mask: String::new(),
        case_sensitive: false,
        global_search: false,
        max_results: Some(50),
        search_in_comments: false,
        search_in_definitions: false,
        parent_schema: Some("dbo".to_string()),
        parent_name: Some(table.clone()),
        match_mode: Some(CompletionAssistantMatchMode::Prefix),
    };
    let completion = native::completion_assistant_search(&mut client, &request).await;
    let filtered_completion = native::completion_assistant_search(
        &mut client,
        &CompletionAssistantRequest { mask: "na".to_string(), ..request },
    )
    .await;

    native::execute_simple_batch_with_max_rows(&mut client, &format!("DROP TABLE dbo.[{table}]"), Some(1))
        .await
        .expect("drop test table");

    let columns: Vec<(String, Option<String>)> =
        columns.expect("get_columns").into_iter().map(|column| (column.name, column.comment)).collect();
    println!("columns: {columns:?}");
    assert_eq!(
        columns,
        vec![("id".to_string(), None), ("name".to_string(), Some("用户名称".to_string())), ("code".to_string(), None),]
    );
    let completion = completion.expect("completion assistant columns");
    assert!(!completion.incomplete);
    assert_eq!(
        completion.candidates.into_iter().map(|column| (column.name, column.comment)).collect::<Vec<_>>(),
        columns,
        "autocomplete preserves column order and the same comments as canonical metadata"
    );
    let filtered_completion = filtered_completion.expect("prefix-filtered completion assistant columns");
    assert_eq!(filtered_completion.candidates.len(), 1);
    let candidate = &filtered_completion.candidates[0];
    assert_eq!(candidate.name, "name");
    assert_eq!(candidate.comment.as_deref(), Some("用户名称"));
    assert_eq!(candidate.schema.as_deref(), Some("dbo"));
    assert_eq!(candidate.parent_name.as_deref(), Some(table.as_str()));
    assert_eq!(table_comment.expect("get_table_comment").as_deref(), Some("table comment"));
    let listed = tables.expect("list_tables").into_iter().find(|info| info.name == table).expect("table listed");
    assert_eq!(listed.comment.as_deref(), Some("table comment"));
    let object = objects.expect("list_objects").into_iter().find(|info| info.name == table).expect("object listed");
    assert_eq!(object.comment.as_deref(), Some("table comment"));
}
