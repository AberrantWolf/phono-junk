//! Owned catalog session and bounded background-job supervisor.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use phono_junk_catalog::{Asset, Id, LibraryFolder};
use phono_junk_db::{SchemaError, aggregate, crud, evidence, open_database, reset_database};
use rusqlite::Connection;

use crate::detail::{AlbumDetail, DetailError, UnidentifiedDetail};
use crate::extract::{ExportError, ExportedDisc};
use crate::identify::{IdentifiedDisc, IdentifyError};
use crate::list::{ListEntry, ListRow};
use crate::scan::{
    IdentificationDisposition, IngestOutcome, ScanError, ScanEvent, ScanRequest, identify_one,
    ingest_path,
};
use crate::verify::{VerifyError, VerifyTarget};
use crate::{PhonoContext, ScanSummary};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionGeneration(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEventKind {
    Started { total: usize },
    Progress { completed: usize, total: usize },
    LibraryChanged,
    Finished,
    Cancelled,
    Failed { error: String },
    AssetCached { asset_id: Id, bytes: Vec<u8> },
    AssetCacheFailed { asset_id: Id, error: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobEvent {
    pub session_generation: SessionGeneration,
    pub job_id: JobId,
    pub kind: JobEventKind,
}

enum JobRequest {
    IdentifyBatch {
        job_id: JobId,
        rip_file_ids: Vec<Id>,
        force_refresh: bool,
    },
    VerifyBatch {
        job_id: JobId,
        disc_ids: Vec<Id>,
    },
    Scan {
        job_id: JobId,
        root: PathBuf,
        request: ScanRequest,
    },
    ExportBatch {
        job_id: JobId,
        disc_ids: Vec<Id>,
        library_root: PathBuf,
    },
    CacheAsset {
        job_id: JobId,
        asset: Asset,
        cache_dir: PathBuf,
    },
    Shutdown,
}

/// One sequential catalog-mutating worker. Provider concurrency remains an
/// implementation detail within a single identification stage.
pub struct JobSupervisor {
    generation: SessionGeneration,
    sender: Option<mpsc::SyncSender<JobRequest>>,
    events: mpsc::Receiver<JobEvent>,
    cancel: Arc<AtomicBool>,
    next_job_id: AtomicU64,
    worker: Option<JoinHandle<()>>,
}

impl JobSupervisor {
    fn new(db_path: PathBuf, context: Arc<PhonoContext>, generation: SessionGeneration) -> Self {
        let (sender, receiver) = mpsc::sync_channel(256);
        let (event_sender, events) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker = thread::spawn(move || {
            let conn = match open_database(&db_path) {
                Ok(conn) => conn,
                Err(error) => {
                    let _ = event_sender.send(JobEvent {
                        session_generation: generation,
                        job_id: JobId(0),
                        kind: JobEventKind::Failed {
                            error: error.to_string(),
                        },
                    });
                    return;
                }
            };
            if let Err(error) = evidence::recover_interrupted_work(&conn) {
                let _ = event_sender.send(JobEvent {
                    session_generation: generation,
                    job_id: JobId(0),
                    kind: JobEventKind::Failed {
                        error: error.to_string(),
                    },
                });
                return;
            }

            while let Ok(request) = receiver.recv() {
                if matches!(request, JobRequest::Shutdown) {
                    break;
                }
                match request {
                    JobRequest::IdentifyBatch {
                        job_id,
                        rip_file_ids,
                        force_refresh,
                    } => run_batch(
                        generation,
                        job_id,
                        &rip_file_ids,
                        &worker_cancel,
                        &event_sender,
                        |rip_file_id| {
                            identify_one(&context, &conn, *rip_file_id, force_refresh)
                                .map(|_| ())
                                .map_err(|error| error.to_string())
                        },
                    ),
                    JobRequest::VerifyBatch { job_id, disc_ids } => run_batch(
                        generation,
                        job_id,
                        &disc_ids,
                        &worker_cancel,
                        &event_sender,
                        |disc_id| {
                            context
                                .verify_disc(&conn, VerifyTarget::DiscId(*disc_id))
                                .map(|_| ())
                                .map_err(|error| error.to_string())
                        },
                    ),
                    JobRequest::Scan {
                        job_id,
                        root,
                        request,
                    } => run_scan_job(
                        generation,
                        job_id,
                        &root,
                        request,
                        &context,
                        &conn,
                        &worker_cancel,
                        &event_sender,
                    ),
                    JobRequest::ExportBatch {
                        job_id,
                        disc_ids,
                        library_root,
                    } => run_batch(
                        generation,
                        job_id,
                        &disc_ids,
                        &worker_cancel,
                        &event_sender,
                        |disc_id| {
                            context
                                .export_disc(&conn, *disc_id, &library_root)
                                .map(|_| ())
                                .map_err(|error| error.to_string())
                        },
                    ),
                    JobRequest::CacheAsset {
                        job_id,
                        asset,
                        cache_dir,
                    } => run_asset_cache_job(
                        generation,
                        job_id,
                        &asset,
                        &cache_dir,
                        &context,
                        &conn,
                        &event_sender,
                    ),
                    JobRequest::Shutdown => unreachable!(),
                }
            }
        });
        Self {
            generation,
            sender: Some(sender),
            events,
            cancel,
            next_job_id: AtomicU64::new(1),
            worker: Some(worker),
        }
    }

    pub fn generation(&self) -> SessionGeneration {
        self.generation
    }

    pub fn queue_identification(
        &self,
        rip_file_ids: Vec<Id>,
        force_refresh: bool,
    ) -> Result<JobId, SessionError> {
        let job_id = self.next_job();
        self.send(JobRequest::IdentifyBatch {
            job_id,
            rip_file_ids,
            force_refresh,
        })?;
        Ok(job_id)
    }

    pub fn queue_verification(&self, disc_ids: Vec<Id>) -> Result<JobId, SessionError> {
        let job_id = self.next_job();
        self.send(JobRequest::VerifyBatch { job_id, disc_ids })?;
        Ok(job_id)
    }

    pub fn queue_scan(&self, root: PathBuf, request: ScanRequest) -> Result<JobId, SessionError> {
        let job_id = self.next_job();
        self.send(JobRequest::Scan {
            job_id,
            root,
            request,
        })?;
        Ok(job_id)
    }

    pub fn queue_export(
        &self,
        disc_ids: Vec<Id>,
        library_root: PathBuf,
    ) -> Result<JobId, SessionError> {
        let job_id = self.next_job();
        self.send(JobRequest::ExportBatch {
            job_id,
            disc_ids,
            library_root,
        })?;
        Ok(job_id)
    }

    pub fn queue_asset_cache(
        &self,
        asset: Asset,
        cache_dir: PathBuf,
    ) -> Result<JobId, SessionError> {
        let job_id = self.next_job();
        self.send(JobRequest::CacheAsset {
            job_id,
            asset,
            cache_dir,
        })?;
        Ok(job_id)
    }

    pub fn try_events(&self) -> Vec<JobEvent> {
        self.events.try_iter().collect()
    }

    fn next_job(&self) -> JobId {
        JobId(self.next_job_id.fetch_add(1, Ordering::Relaxed))
    }

    fn send(&self, request: JobRequest) -> Result<(), SessionError> {
        self.sender
            .as_ref()
            .ok_or(SessionError::SupervisorClosed)?
            .send(request)
            .map_err(|_| SessionError::SupervisorClosed)
    }

    /// Cancel outstanding work, close the bounded queue, and join the worker.
    pub fn shutdown(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(JobRequest::Shutdown);
            drop(sender);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for JobSupervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_batch<T, F>(
    generation: SessionGeneration,
    job_id: JobId,
    items: &[T],
    cancel: &AtomicBool,
    events: &mpsc::Sender<JobEvent>,
    mut run: F,
) where
    F: FnMut(&T) -> Result<(), String>,
{
    let total = items.len();
    send_event(events, generation, job_id, JobEventKind::Started { total });
    for (index, item) in items.iter().enumerate() {
        if cancel.load(Ordering::Acquire) {
            send_event(events, generation, job_id, JobEventKind::Cancelled);
            return;
        }
        if let Err(error) = run(item) {
            send_event(events, generation, job_id, JobEventKind::Failed { error });
            return;
        }
        send_event(
            events,
            generation,
            job_id,
            JobEventKind::Progress {
                completed: index + 1,
                total,
            },
        );
    }
    // One redraw notification per completed batch, not per catalog row.
    send_event(events, generation, job_id, JobEventKind::LibraryChanged);
    send_event(events, generation, job_id, JobEventKind::Finished);
}

#[allow(clippy::too_many_arguments)]
fn run_scan_job(
    generation: SessionGeneration,
    job_id: JobId,
    root: &Path,
    request: ScanRequest,
    context: &PhonoContext,
    conn: &Connection,
    cancel: &AtomicBool,
    events: &mpsc::Sender<JobEvent>,
) {
    send_event(
        events,
        generation,
        job_id,
        JobEventKind::Started { total: 0 },
    );
    let mut queued = Vec::new();
    let mut completed = 0usize;
    let result = context.scan_library_cancellable(
        conn,
        root,
        request,
        |event| {
            let found = matches!(&event, ScanEvent::Found { .. });
            if let ScanEvent::Ingested {
                rip_file_id,
                state: phono_junk_core::IdentificationState::Queued,
                ..
            } = &event
            {
                queued.push(*rip_file_id);
            }
            if !found {
                completed += 1;
                send_event(
                    events,
                    generation,
                    job_id,
                    JobEventKind::Progress {
                        completed,
                        total: 0,
                    },
                );
            }
        },
        || cancel.load(Ordering::Acquire),
    );
    if let Err(error) = result {
        send_event(
            events,
            generation,
            job_id,
            JobEventKind::Failed {
                error: error.to_string(),
            },
        );
        return;
    }
    if cancel.load(Ordering::Acquire) {
        send_event(events, generation, job_id, JobEventKind::Cancelled);
        return;
    }
    if request.identification == IdentificationDisposition::Queue {
        for rip_file_id in queued {
            if cancel.load(Ordering::Acquire) {
                send_event(events, generation, job_id, JobEventKind::Cancelled);
                return;
            }
            if let Err(error) = identify_one(
                context,
                conn,
                rip_file_id,
                request.refresh == crate::RefreshPolicy::Force,
            ) {
                send_event(
                    events,
                    generation,
                    job_id,
                    JobEventKind::Failed {
                        error: error.to_string(),
                    },
                );
                return;
            }
            completed += 1;
            send_event(
                events,
                generation,
                job_id,
                JobEventKind::Progress {
                    completed,
                    total: 0,
                },
            );
        }
    }
    send_event(events, generation, job_id, JobEventKind::LibraryChanged);
    send_event(events, generation, job_id, JobEventKind::Finished);
}

fn run_asset_cache_job(
    generation: SessionGeneration,
    job_id: JobId,
    asset: &Asset,
    cache_dir: &Path,
    context: &PhonoContext,
    conn: &Connection,
    events: &mpsc::Sender<JobEvent>,
) {
    send_event(
        events,
        generation,
        job_id,
        JobEventKind::Started { total: 1 },
    );
    match crate::cache_asset_bytes(context, conn, asset, cache_dir) {
        Ok(bytes) => {
            send_event(
                events,
                generation,
                job_id,
                JobEventKind::AssetCached {
                    asset_id: asset.id,
                    bytes,
                },
            );
            send_event(events, generation, job_id, JobEventKind::LibraryChanged);
            send_event(events, generation, job_id, JobEventKind::Finished);
        }
        Err(error) => send_event(
            events,
            generation,
            job_id,
            JobEventKind::AssetCacheFailed {
                asset_id: asset.id,
                error: error.to_string(),
            },
        ),
    }
}

fn send_event(
    sender: &mpsc::Sender<JobEvent>,
    generation: SessionGeneration,
    job_id: JobId,
    kind: JobEventKind,
) {
    let _ = sender.send(JobEvent {
        session_generation: generation,
        job_id,
        kind,
    });
}

pub struct LibrarySession {
    db_path: PathBuf,
    connection: Option<Connection>,
    context: Arc<PhonoContext>,
    generation: SessionGeneration,
    supervisor: JobSupervisor,
}

impl LibrarySession {
    pub fn open(
        db_path: impl Into<PathBuf>,
        context: Arc<PhonoContext>,
    ) -> Result<Self, SessionError> {
        let db_path = db_path.into();
        let connection = open_database(&db_path)?;
        evidence::recover_interrupted_work(&connection)?;
        let generation = next_generation();
        let supervisor = JobSupervisor::new(db_path.clone(), Arc::clone(&context), generation);
        Ok(Self {
            db_path,
            connection: Some(connection),
            context,
            generation,
            supervisor,
        })
    }

    pub fn path(&self) -> &Path {
        &self.db_path
    }

    /// Explicitly destroy an alpha catalog and open a fresh v7 session. The
    /// caller must obtain confirmation before invoking this operation.
    pub fn rebuild(
        db_path: impl Into<PathBuf>,
        context: Arc<PhonoContext>,
    ) -> Result<Self, SessionError> {
        let db_path = db_path.into();
        drop(reset_database(&db_path)?);
        Self::open(db_path, context)
    }

    pub fn generation(&self) -> SessionGeneration {
        self.generation
    }

    pub fn connection(&self) -> &Connection {
        self.connection.as_ref().expect("open session connection")
    }

    pub fn context(&self) -> &Arc<PhonoContext> {
        &self.context
    }

    pub fn supervisor(&self) -> &JobSupervisor {
        &self.supervisor
    }

    pub fn scan<F>(
        &self,
        root: &Path,
        request: ScanRequest,
        mut progress: F,
    ) -> Result<ScanSummary, SessionError>
    where
        F: FnMut(ScanEvent<'_>),
    {
        let mut queued = Vec::new();
        let summary = self
            .context
            .scan_library(self.connection(), root, request, |event| {
                if request.identification == IdentificationDisposition::Queue
                    && let ScanEvent::Ingested {
                        rip_file_id,
                        state: phono_junk_core::IdentificationState::Queued,
                        ..
                    } = &event
                {
                    queued.push(*rip_file_id);
                }
                progress(event);
            })?;
        if !queued.is_empty() {
            self.supervisor
                .queue_identification(queued, request.refresh == crate::RefreshPolicy::Force)?;
        }
        Ok(summary)
    }

    pub fn identify_path(
        &self,
        path: &Path,
        request: ScanRequest,
    ) -> Result<IngestOutcome, SessionError> {
        Ok(ingest_path(
            &self.context,
            self.connection(),
            path,
            &request,
        )?)
    }

    pub fn identify_rip(
        &self,
        rip_file_id: Id,
        force_refresh: bool,
    ) -> Result<IdentifiedDisc, SessionError> {
        Ok(identify_one(
            &self.context,
            self.connection(),
            rip_file_id,
            force_refresh,
        )?)
    }

    pub fn queued_rip_ids(&self) -> Result<Vec<Id>, SessionError> {
        use phono_junk_core::IdentificationState;
        Ok(crud::list_rip_files_by_state(
            self.connection(),
            &[IdentificationState::Queued, IdentificationState::Failed],
        )?
        .into_iter()
        .map(|rip| rip.id)
        .collect())
    }

    pub fn verify(&self, target: VerifyTarget) -> Result<crate::VerifySummary, SessionError> {
        Ok(self.context.verify_disc(self.connection(), target)?)
    }

    pub fn export_disc(
        &self,
        disc_id: Id,
        library_root: &Path,
    ) -> Result<ExportedDisc, SessionError> {
        Ok(self
            .context
            .export_disc(self.connection(), disc_id, library_root)?)
    }

    pub fn plan_export_disc(
        &self,
        disc_id: Id,
        library_root: &Path,
    ) -> Result<ExportedDisc, SessionError> {
        Ok(self
            .context
            .plan_export_disc(self.connection(), disc_id, library_root)?)
    }

    pub fn list_rows(&self) -> Result<Vec<ListRow>, SessionError> {
        Ok(crate::load_list_rows(self.connection())?)
    }

    pub fn list_entries(&self) -> Result<Vec<ListEntry>, SessionError> {
        Ok(crate::load_list_entries(self.connection())?)
    }

    pub fn tracked_folders(&self) -> Result<Vec<LibraryFolder>, SessionError> {
        Ok(crud::list_library_folders(self.connection())?)
    }

    pub fn track_folder(&self, path: &Path) -> Result<Id, SessionError> {
        Ok(crud::insert_library_folder(self.connection(), path)?)
    }

    pub fn disc_ids_for_albums(&self, album_ids: &[Id]) -> Result<Vec<Id>, SessionError> {
        let mut ids = Vec::new();
        for &album_id in album_ids {
            let aggregate = aggregate::load_album(self.connection(), album_id)?
                .ok_or(DetailError::AlbumMissing(album_id))?;
            ids.extend(aggregate.discs.into_iter().map(|disc| disc.id));
        }
        Ok(ids)
    }

    pub fn rip_ids_for_albums(&self, album_ids: &[Id]) -> Result<Vec<Id>, SessionError> {
        let mut ids = Vec::new();
        for &album_id in album_ids {
            let aggregate = aggregate::load_album(self.connection(), album_id)?
                .ok_or(DetailError::AlbumMissing(album_id))?;
            ids.extend(aggregate.rip_files.into_iter().map(|rip| rip.id));
        }
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }

    pub fn audit_missing_redumper(&self) -> Result<Vec<crate::audit::AuditRow>, SessionError> {
        Ok(crate::audit::list_missing_redumper(self.connection())?)
    }

    pub fn audit_summary(&self) -> Result<crate::audit::AuditSummary, SessionError> {
        Ok(crate::audit::summarize(self.connection())?)
    }

    pub fn album_detail(&self, album_id: Id) -> Result<AlbumDetail, SessionError> {
        Ok(crate::load_album_detail(self.connection(), album_id)?)
    }

    pub fn unidentified_detail(&self, rip_file_id: Id) -> Result<UnidentifiedDetail, SessionError> {
        let rip = crud::get_rip_file(self.connection(), rip_file_id)?
            .ok_or(SessionError::RipFileMissing(rip_file_id))?;
        Ok(crate::load_unidentified_detail(rip))
    }

    pub fn album_summary_for_disc(
        &self,
        disc_id: Id,
    ) -> Result<Option<AlbumSummary>, SessionError> {
        Ok(
            aggregate::load_for_disc(self.connection(), disc_id)?.map(|aggregate| {
                let release_id = aggregate
                    .discs
                    .iter()
                    .find(|disc| disc.id == disc_id)
                    .map(|disc| disc.release_id);
                AlbumSummary {
                    album_id: aggregate.album.id,
                    release_id,
                    title: aggregate.album.title,
                    artist: aggregate.album.artist_credit,
                    year: aggregate.album.year,
                }
            }),
        )
    }

    pub fn switch_database(&mut self, db_path: impl Into<PathBuf>) -> Result<(), SessionError> {
        self.reopen(db_path.into(), false)
    }

    /// The caller owns explicit destructive confirmation.
    pub fn reset_database(&mut self) -> Result<(), SessionError> {
        self.reopen(self.db_path.clone(), true)
    }

    fn reopen(&mut self, db_path: PathBuf, reset: bool) -> Result<(), SessionError> {
        self.supervisor.shutdown();
        drop(self.connection.take());
        self.generation = next_generation();
        let connection = if reset {
            reset_database(&db_path)?
        } else {
            open_database(&db_path)?
        };
        evidence::recover_interrupted_work(&connection)?;
        self.db_path = db_path;
        self.supervisor = JobSupervisor::new(
            self.db_path.clone(),
            Arc::clone(&self.context),
            self.generation,
        );
        self.connection = Some(connection);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AlbumSummary {
    pub album_id: Id,
    pub release_id: Option<Id>,
    pub title: String,
    pub artist: Option<String>,
    pub year: Option<u16>,
}

fn next_generation() -> SessionGeneration {
    SessionGeneration(NEXT_GENERATION.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Schema(#[from] SchemaError),
    #[error(transparent)]
    Db(#[from] phono_junk_db::DbError),
    #[error(transparent)]
    Scan(#[from] ScanError),
    #[error(transparent)]
    Verify(#[from] VerifyError),
    #[error(transparent)]
    Identify(#[from] IdentifyError),
    #[error(transparent)]
    Export(#[from] ExportError),
    #[error(transparent)]
    Detail(#[from] DetailError),
    #[error("rip file {0} not found")]
    RipFileMissing(Id),
    #[error("job supervisor is closed")]
    SupervisorClosed,
}
