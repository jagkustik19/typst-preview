use std::path::Path;
use std::sync::Arc;

use log::error;

use typst::diag::SourceResult;
use typst::layout::Position;
use typst::model::Document;
use typst::syntax::Span;

use typst_ts_compiler::service::{
    CompileActor, CompileClient as TsCompileClient, CompileExporter, Compiler, DocToSrcJumpInfo,
    WorldExporter,
};
use typst_ts_compiler::service::{CompileDriver, CompileMiddleware};
use typst_ts_compiler::vfs::notify::{FileChangeSet, MemoryEvent};
use typst_ts_core::debug_loc::SourceSpanOffset;
use typst_ts_core::error::prelude::ZResult;

use typst_preview::Location;
use typst_preview::{CompilationHandle, CompileStatus};
use typst_preview::{CompileHost, EditorServer, MemoryFiles, MemoryFilesShort, SourceFileServer};

pub type CompileService<H> = CompileActor<Reporter<CompileExporter<CompileDriver>, H>>;
pub type CompileClient<H> = TsCompileClient<CompileService<H>>;

pub struct CompileServer<H: CompilationHandle> {
    inner: CompileService<H>,
    client: TypstClient<H>,
}

pub struct Reporter<C, H> {
    inner: C,
    cb: H,
}

impl<C: Compiler, H: CompilationHandle> CompileMiddleware for Reporter<C, H> {
    type Compiler = C;

    fn inner(&self) -> &Self::Compiler {
        &self.inner
    }

    fn inner_mut(&mut self) -> &mut Self::Compiler {
        &mut self.inner
    }

    fn wrap_compile(
        &mut self,
        env: &mut typst_ts_compiler::service::CompileEnv,
    ) -> SourceResult<Arc<Document>> {
        self.cb.status(CompileStatus::Compiling);
        match self.inner_mut().compile(env) {
            Ok(doc) => {
                self.cb.notify_compile(Ok(doc.clone()));
                Ok(doc)
            }
            Err(err) => {
                self.cb.notify_compile(Err(CompileStatus::CompileError));
                Err(err)
            }
        }
    }
}

impl<C: Compiler + WorldExporter, H> WorldExporter for Reporter<C, H> {
    fn export(&mut self, output: Arc<typst::model::Document>) -> SourceResult<()> {
        self.inner.export(output)
    }
}

impl<H: CompilationHandle> CompileServer<H> {
    pub fn new(
        compiler_driver: CompileDriver,
        cb: H,
        // renderer_sender: broadcast::Sender<RenderActorRequest>,
        // editor_conn_sender: mpsc::UnboundedSender<EditorActorRequest>,
    ) -> Self {
        // CompileExporter + DynamicLayoutCompiler + WatchDriver
        let root = compiler_driver.world.root.clone();
        // let r = renderer_sender.clone();
        let driver = CompileExporter::new(compiler_driver);
        let driver = Reporter { inner: driver, cb };
        let inner = CompileActor::new(driver, root.as_ref().to_owned()).with_watch(true);

        Self {
            inner,
            client: TypstClient {
                inner: once_cell::sync::OnceCell::new(),
            },
        }
    }

    pub fn spawn(self) -> ZResult<TypstClient<H>> {
        let (server, client) = self.inner.split();
        tokio::spawn(server.spawn());

        self.client.inner.set(client).ok().unwrap();

        Ok(self.client)
    }
}

pub struct TypstClient<H: CompilationHandle> {
    inner: once_cell::sync::OnceCell<CompileClient<H>>,
}

impl<H: CompilationHandle> TypstClient<H> {
    fn inner(&mut self) -> &mut CompileClient<H> {
        self.inner.get_mut().unwrap()
    }
}

impl<H: CompilationHandle> SourceFileServer for TypstClient<H> {
    async fn resolve_source_span(&mut self, loc: Location) -> ZResult<Option<SourceSpanOffset>> {
        let Location::Src(src_loc) = loc;
        self.inner().resolve_src_location(src_loc).await
    }

    async fn resolve_document_position(&mut self, loc: Location) -> ZResult<Option<Position>> {
        let Location::Src(src_loc) = loc;

        let path = Path::new(&src_loc.filepath).to_owned();
        let line = src_loc.pos.line;
        let column = src_loc.pos.column;

        self.inner()
            .resolve_src_to_doc_jump(path, line, column)
            .await
    }

    async fn resolve_source_location(
        &mut self,
        s: Span,
        offset: Option<usize>,
    ) -> ZResult<Option<DocToSrcJumpInfo>> {
        Ok(self
            .inner()
            .resolve_span_and_offset(s, offset)
            .await
            .map_err(|err| {
                error!("TypstActor: failed to resolve doc to src jump: {:#}", err);
            })
            .ok()
            .flatten())
    }
}

impl<H: CompilationHandle> EditorServer for TypstClient<H> {
    async fn update_memory_files(&mut self, files: MemoryFiles, reset_shadow: bool) -> ZResult<()> {
        // todo: is it safe to believe that the path is normalized?
        let now = std::time::SystemTime::now();
        let files = FileChangeSet::new_inserts(
            files
                .files
                .into_iter()
                .map(|(path, content)| {
                    let content = content.as_bytes().into();
                    // todo: cloning PathBuf -> Arc<Path>
                    (path.into(), Ok((now, content)).into())
                })
                .collect(),
        );
        self.inner().add_memory_changes(if reset_shadow {
            MemoryEvent::Sync(files)
        } else {
            MemoryEvent::Update(files)
        });

        Ok(())
    }

    async fn remove_shadow_files(&mut self, files: MemoryFilesShort) -> ZResult<()> {
        // todo: is it safe to believe that the path is normalized?
        let files = FileChangeSet::new_removes(files.files.into_iter().map(From::from).collect());
        self.inner().add_memory_changes(MemoryEvent::Update(files));

        Ok(())
    }
}

impl<H: CompilationHandle> CompileHost for TypstClient<H> {}
