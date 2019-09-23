use std::sync::Arc;

use futures_core::stream::BoxStream;
use futures_util::{future, stream::{self, StreamExt}, TryStreamExt};
use log::info;

use crate::{
    ClientHandle,
    errors::Result,
    types::{
        Block,
        block::BlockRef,
        Cmd,
        Query,
        query_result::stream_blocks::BlockStream,
        Row,
        Rows
    },
};

pub(crate) mod stream_blocks;

/// Result of a query or statement execution.
pub struct QueryResult<'a> {
    pub(crate) client: &'a mut ClientHandle,
    pub(crate) query: Query,
}

impl<'a> QueryResult<'a> {
    /// Fetch data from table. It returns a block that contains all rows.
    pub async fn fetch_all(self) -> Result<Block> {
        let blocks = self.stream_blocks().try_fold(Vec::new(), |mut blocks, block| {
            if !block.is_empty() {
                blocks.push(block);
            }
            future::ready(Ok(blocks))
        }).await?;
        Ok(Block::concat(blocks.as_slice()))
    }

    /// Method that produces a stream of blocks containing rows
    ///
    /// example:
    ///
    /// ```rust
    /// # use std::env;
    /// # use clickhouse_rs::{Pool, errors::Result};
    /// # use futures_util::{future, TryStreamExt};
    /// #
    /// # let rt = tokio::runtime::Runtime::new().unwrap();
    /// # let ret: Result<()> = rt.block_on(async {
    /// #
    /// #     let database_url = env::var("DATABASE_URL")
    /// #         .unwrap_or("tcp://localhost:9000?compression=lz4".into());
    /// #
    /// #     let sql_query = "SELECT number FROM system.numbers LIMIT 100000";
    /// #     let pool = Pool::new(database_url);
    /// #
    ///       let mut c = pool.get_handle().await?;
    ///       let mut result = c.query(sql_query)
    ///           .stream_blocks()
    ///           .try_for_each(|block| {
    ///               println!("{:?}\nblock counts: {} rows", block, block.row_count());
    ///               future::ready(Ok(()))
    ///           }).await?;
    /// #     Ok(())
    /// # });
    /// # ret.unwrap()
    /// ```
    pub fn stream_blocks(self) -> BoxStream<'a, Result<Block>> {
        let query = self.query.clone();

        self.client.wrap_stream::<'a, _>(move |c: &'a mut ClientHandle| {
            info!("[send query] {}", query.get_sql());
            c.pool.detach();

            let context = c.context.clone();

            let inner = c.inner
                .take()
                .unwrap()
                .call(Cmd::SendQuery(query, context.clone()));

            BlockStream::<'a>::new(c, inner)
        })
    }

    /// Method that produces a stream of rows
    pub fn stream(self) -> BoxStream<'a, Result<Row<'a>>> {
        Box::pin(self.stream_blocks()
            .map(|block_ret| {
                let result: BoxStream<'a, Result<Row<'a>>> =
                    match block_ret {
                        Ok(block) => {
                            let block = Arc::new(block);
                            let block_ref = BlockRef::Owned(block);

                            Box::pin(stream::iter(Rows { row: 0, block_ref })
                                .map(|row| -> Result<Row<'static>> { Ok(row) }))
                        }
                        Err(err) => Box::pin(stream::once(future::err(err))),
                    };
                result
            }).flatten())
    }
}
