use std::{
    collections::{HashMap, VecDeque},
    fs::{File, Metadata, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use super::{
    index::{Index, Location, Span, invalid, validate_manifest, validate_record},
    *,
};

/// Explicit-path streaming producer. Creating a writer never overwrites a file.
/// A dropped/failed writer leaves an incomplete bundle, which readers reject.
pub struct BundleWriter {
    path: PathBuf,
    writer: BufWriter<File>,
    manifest: Manifest,
    limits: Limits,
    order: Order,
    counts: Counts,
    bytes: u64,
}

impl BundleWriter {
    pub fn create(
        path: impl AsRef<Path>,
        manifest: Manifest,
        limits: Limits,
    ) -> Result<Self, BundleError> {
        limits.validate()?;
        validate_manifest(&manifest)?;
        let encoded = encode(&BundleRecord::Manifest(manifest.clone()), &limits)?;
        if encoded.len() as u64 > limits.max_bundle_bytes {
            return Err(BundleError::Limit("bundle bytes"));
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.as_ref())?;
        if !file.metadata()?.is_file() {
            return Err(invalid("bundle output must be a new regular file"));
        }
        let mut writer = BufWriter::new(file);
        writer.write_all(&encoded)?;
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            writer,
            manifest,
            limits,
            order: Order::default(),
            counts: Counts::default(),
            bytes: encoded.len() as u64,
        })
    }

    /// Supply evidence first, then entities, then relations; within each section
    /// IDs must be strictly increasing. Only the current record is serialized.
    pub fn push(&mut self, record: &BundleRecord) -> Result<(), BundleError> {
        validate_record(record, &self.manifest)?;
        self.order.check(record)?;
        let mut counts = self.counts;
        match record {
            BundleRecord::Evidence(_) => counts.evidence += 1,
            BundleRecord::Entity(_) => counts.entities += 1,
            BundleRecord::Relation(_) => counts.relations += 1,
            _ => return Err(invalid("only semantic records can be pushed")),
        }
        if counts.entities > self.limits.max_entities
            || counts.relations > self.limits.max_relations
            || counts.evidence > self.limits.max_evidence
        {
            return Err(BundleError::Limit("record count"));
        }
        let encoded = encode(record, &self.limits)?;
        if self.bytes + encoded.len() as u64 > self.limits.max_bundle_bytes {
            return Err(BundleError::Limit("bundle bytes"));
        }
        self.writer.write_all(&encoded)?;
        self.bytes += encoded.len() as u64;
        self.counts = counts;
        self.order.accept(record);
        Ok(())
    }

    /// Finalize, then validate/index the completed file before returning it.
    /// Semantic validation failure leaves the file available for diagnostics.
    pub fn finish(self) -> Result<LocalBundle, BundleError> {
        self.finish_with_progress(|_| ControlFlow::Continue(()))
    }

    pub fn finish_with_progress(
        mut self,
        progress: impl FnMut(IndexProgress) -> ControlFlow<()>,
    ) -> Result<LocalBundle, BundleError> {
        let encoded = encode(&BundleRecord::Complete(self.counts), &self.limits)?;
        if self.bytes + encoded.len() as u64 > self.limits.max_bundle_bytes {
            return Err(BundleError::Limit("bundle bytes"));
        }
        self.writer.write_all(&encoded)?;
        self.writer.flush()?;
        self.writer.get_ref().sync_data()?;
        drop(self.writer);
        LocalBundle::open_with_progress(&self.path, self.limits, progress)
    }
}

#[derive(Default)]
struct Order {
    phase: u8,
    last_id: String,
}

impl Order {
    fn check(&self, record: &BundleRecord) -> Result<(), BundleError> {
        let (id, phase) = record
            .identity()
            .ok_or_else(|| invalid("unexpected record"))?;
        if phase < self.phase || (phase == self.phase && id <= self.last_id.as_str()) {
            return Err(invalid(
                "sections and record IDs must be in canonical increasing order",
            ));
        }
        Ok(())
    }

    fn accept(&mut self, record: &BundleRecord) {
        let (id, phase) = record.identity().expect("checked semantic record");
        self.last_id.clear();
        self.last_id.push_str(id);
        self.phase = phase;
    }
}

#[derive(PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    modified: SystemTime,
}

impl Fingerprint {
    fn of(metadata: Metadata) -> Result<Self, BundleError> {
        if !metadata.is_file() {
            return Err(invalid("bundle input must be a regular file"));
        }
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

struct CachedDetail {
    detail: Arc<EntityDetail>,
    charged_bytes: usize,
}

/// Immutable revision backed by one regular file. The metadata index is resident;
/// full entity detail is read on demand and retained only within cache budgets.
pub struct LocalBundle {
    file: File,
    fingerprint: Fingerprint,
    manifest: Manifest,
    limits: Limits,
    index: Index,
    cache: HashMap<usize, CachedDetail>,
    lru: VecDeque<usize>,
    cached_bytes: usize,
}

impl LocalBundle {
    pub fn open(path: impl AsRef<Path>, limits: Limits) -> Result<Self, BundleError> {
        Self::open_with_progress(path, limits, |_| ControlFlow::Continue(()))
    }

    /// Indexing is a separate cancellable phase. No provider is published until
    /// every record, ownership relation, and completion marker has been validated.
    /// Callbacks continue during final validation with the completed byte/record
    /// counts; reaching EOF alone does not mean the index is ready to publish.
    pub fn open_with_progress(
        path: impl AsRef<Path>,
        limits: Limits,
        mut progress: impl FnMut(IndexProgress) -> ControlFlow<()>,
    ) -> Result<Self, BundleError> {
        limits.validate()?;
        let file = open_regular(path.as_ref())?;
        let fingerprint = Fingerprint::of(file.metadata()?)?;
        if fingerprint.len > limits.max_bundle_bytes {
            return Err(BundleError::Limit("bundle bytes"));
        }
        let mut reader = BufReader::new(file);
        let mut bytes = Vec::new();
        let mut offset = 0;
        let mut indexed = 0;
        let mut report = |offset, indexed| {
            if progress(IndexProgress {
                bytes_read: offset,
                total_bytes: fingerprint.len,
                records_indexed: indexed,
            })
            .is_break()
            {
                Err(BundleError::Cancelled)
            } else {
                Ok(())
            }
        };
        report(0, 0)?;
        if !read_line(&mut reader, &mut bytes, limits.max_record_bytes)? {
            return Err(invalid("empty bundle"));
        }
        offset += bytes.len() as u64;
        let BundleRecord::Manifest(manifest) = decode(&bytes)? else {
            return Err(invalid("first record must be a manifest"));
        };
        validate_manifest(&manifest)?;
        let mut index = Index::new();
        let mut order = Order::default();
        let mut complete = false;
        while read_line(&mut reader, &mut bytes, limits.max_record_bytes)? {
            let len = bytes.len();
            let next = offset + len as u64;
            if next > limits.max_bundle_bytes || next > fingerprint.len {
                return Err(BundleError::SourceChanged);
            }
            if complete {
                return Err(invalid("data after completion marker"));
            }
            let record = decode(&bytes)?;
            match record {
                BundleRecord::Complete(counts) => {
                    if counts != index.counts {
                        return Err(invalid("completion counts disagree with records"));
                    }
                    complete = true;
                }
                _ => {
                    order.check(&record)?;
                    index.add(
                        &record,
                        Span {
                            offset,
                            len,
                            checksum: checksum(&bytes),
                        },
                        &manifest,
                        &limits,
                    )?;
                    order.accept(&record);
                    indexed += 1;
                }
            }
            offset = next;
            if indexed % 1024 == 0 {
                report(offset, indexed)?;
            }
        }
        if !complete || offset != fingerprint.len {
            return Err(invalid("incomplete bundle"));
        }
        report(offset, indexed)?;
        index.finish(&manifest, &limits, || report(offset, indexed))?;
        report(offset, indexed)?;
        let file = reader.into_inner();
        if Fingerprint::of(file.metadata()?)? != fingerprint {
            return Err(BundleError::SourceChanged);
        }
        Ok(Self {
            file,
            fingerprint,
            manifest,
            limits,
            index,
            cache: HashMap::new(),
            lru: VecDeque::new(),
            cached_bytes: 0,
        })
    }

    pub fn stats(&self) -> ProviderStats {
        ProviderStats {
            counts: self.index.counts,
            estimated_index_bytes: self.index.estimated_bytes,
            cached_records: self.cache.len(),
            cached_bytes: self.cached_bytes,
        }
    }

    pub fn clear_detail_cache(&mut self) {
        self.cache.clear();
        self.lru.clear();
        self.cached_bytes = 0;
    }

    fn check(&self, revision: u64) -> Result<(), BundleError> {
        if revision != self.manifest.revision {
            return Err(BundleError::StaleRevision {
                expected: revision,
                actual: self.manifest.revision,
            });
        }
        if Fingerprint::of(self.file.metadata()?)? != self.fingerprint {
            return Err(BundleError::SourceChanged);
        }
        Ok(())
    }

    fn lookup(&self, id: &str) -> Result<usize, BundleError> {
        self.index
            .entity_index(id)
            .ok_or_else(|| BundleError::NotFound(id.to_owned()))
    }

    fn read(&mut self, span: Span) -> Result<BundleRecord, BundleError> {
        let mut bytes = vec![0; span.len];
        // Indexed records have absolute offsets. Positional reads avoid a
        // separate seek for every relationship in a bounded projection page.
        // Read fresh bytes on every call; retain the per-record checksum guard.
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.file.read_exact_at(&mut bytes, span.offset)?;
        }
        #[cfg(not(unix))]
        {
            use std::io::{Read, Seek, SeekFrom};
            self.file.seek(SeekFrom::Start(span.offset))?;
            self.file.read_exact(&mut bytes)?;
        }
        if checksum(&bytes) != span.checksum {
            return Err(BundleError::SourceChanged);
        }
        decode(&bytes)
    }

    fn entity(&mut self, i: usize) -> Result<EntityRecord, BundleError> {
        match self.read(self.index.entities[i].span)? {
            BundleRecord::Entity(e) => Ok(e),
            _ => Err(BundleError::SourceChanged),
        }
    }

    fn page_start(
        &self,
        query: &str,
        request: &PageRequest,
        total: usize,
    ) -> Result<usize, BundleError> {
        if request.limit == 0 || request.limit > self.limits.max_page_records {
            return Err(BundleError::Limit("page record count"));
        }
        if let Some(cursor) = &request.after {
            if cursor.model_id != self.manifest.model_id
                || cursor.revision != self.manifest.revision
                || cursor.query != query
                || cursor.offset > total
            {
                return Err(BundleError::InvalidCursor);
            }
            Ok(cursor.offset)
        } else {
            Ok(0)
        }
    }

    fn next(&self, query: String, offset: usize, total: usize) -> Option<PageCursor> {
        (offset < total).then(|| PageCursor {
            model_id: self.manifest.model_id.clone(),
            revision: self.manifest.revision,
            query,
            offset,
        })
    }

    fn touch(&mut self, i: usize) {
        self.lru.retain(|entry| *entry != i);
        self.lru.push_back(i);
    }

    fn retain_detail(&mut self, i: usize, detail: Arc<EntityDetail>) {
        // Include room for String/Vec headers, bounded list capacities, Arc,
        // cache bookkeeping and allocator rounding as well as serialized bytes.
        let charged_bytes = self.index.entities[i].span.len + 4096;
        if self.limits.cache_records == 0 || charged_bytes > self.limits.cache_bytes {
            return;
        }
        while self.cache.len() >= self.limits.cache_records
            || self.cached_bytes + charged_bytes > self.limits.cache_bytes
        {
            let oldest = self.lru.pop_front().expect("nonempty charged cache");
            self.cached_bytes -= self
                .cache
                .remove(&oldest)
                .expect("cached LRU entry")
                .charged_bytes;
        }
        self.cache.insert(
            i,
            CachedDetail {
                detail,
                charged_bytes,
            },
        );
        self.cached_bytes += charged_bytes;
        self.touch(i);
    }
}

impl ModelProvider for LocalBundle {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn summary(&mut self, revision: u64, id: &str) -> Result<Option<EntitySummary>, BundleError> {
        self.check(revision)?;
        self.index
            .entity_index(id)
            .map(|i| self.entity(i).map(|e| e.summary))
            .transpose()
    }

    fn owner_id(&self, revision: u64, id: &str) -> Result<Option<String>, BundleError> {
        self.check(revision)?;
        Ok(self.index.entities[self.lookup(id)?]
            .parent
            .map(|owner| self.index.entities[owner].id.to_string()))
    }

    fn in_lens(&self, revision: u64, lens: SemanticLens, id: &str) -> Result<bool, BundleError> {
        self.check(revision)?;
        Ok(self
            .index
            .entity_index(id)
            .is_some_and(|i| self.index.in_lens(lens, i)))
    }

    fn owner_id_in(
        &self,
        revision: u64,
        lens: SemanticLens,
        id: &str,
    ) -> Result<Option<String>, BundleError> {
        self.check(revision)?;
        let i = self.lookup(id)?;
        if !self.index.in_lens(lens, i) {
            return Err(invalid("entity is outside the selected semantic lens"));
        }
        Ok(self
            .index
            .parent_in(Some(lens), i)
            .map(|owner| self.index.entities[owner].id.to_string()))
    }

    fn relation(&mut self, revision: u64, id: &str) -> Result<Option<RelationRecord>, BundleError> {
        self.check(revision)?;
        let Some(Location::Relation(i)) = self.index.ids.get(id) else {
            return Ok(None);
        };
        match self.read(self.index.relations[*i].span)? {
            BundleRecord::Relation(relation) => Ok(Some(relation)),
            _ => Err(BundleError::SourceChanged),
        }
    }

    fn children(
        &mut self,
        revision: u64,
        id: &str,
        request: PageRequest,
    ) -> Result<Page<EntitySummary>, BundleError> {
        self.check(revision)?;
        let parent = self.lookup(id)?;
        let query = format!("children:{id}");
        let total = self.index.entities[parent].children.len();
        let mut offset = self.page_start(&query, &request, total)?;
        let mut items = Vec::new();
        let mut bytes = 0;
        while offset < total && items.len() < request.limit {
            let i = self.index.entities[parent].children[offset];
            let len = self.index.entities[i].span.len;
            if bytes + len > self.limits.max_page_bytes {
                break;
            }
            items.push(self.entity(i)?.summary);
            bytes += len;
            offset += 1;
        }
        Ok(Page {
            revision,
            items,
            next: self.next(query, offset, total),
        })
    }

    fn incident(
        &mut self,
        revision: u64,
        id: &str,
        request: PageRequest,
    ) -> Result<Page<RelationRecord>, BundleError> {
        self.check(revision)?;
        let entity = self.lookup(id)?;
        let query = format!("incident:{id}");
        let total = self.index.entities[entity].incident.len();
        let mut offset = self.page_start(&query, &request, total)?;
        let mut items = Vec::new();
        let mut bytes = 0;
        while offset < total && items.len() < request.limit {
            let i = self.index.entities[entity].incident[offset];
            let span = self.index.relations[i].span;
            if bytes + span.len > self.limits.max_page_bytes {
                break;
            }
            let BundleRecord::Relation(relation) = self.read(span)? else {
                return Err(BundleError::SourceChanged);
            };
            items.push(relation);
            bytes += span.len;
            offset += 1;
        }
        Ok(Page {
            revision,
            items,
            next: self.next(query, offset, total),
        })
    }

    fn children_at(
        &mut self,
        revision: u64,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Page<EntitySummary>, BundleError> {
        self.check(revision)?;
        let query = format!("children:{id}");
        self.children(
            revision,
            id,
            PageRequest {
                limit,
                after: Some(PageCursor {
                    model_id: self.manifest.model_id.clone(),
                    revision,
                    query,
                    offset,
                }),
            },
        )
    }

    fn child_count(&self, revision: u64, id: &str) -> Result<usize, BundleError> {
        self.check(revision)?;
        Ok(self.index.entities[self.lookup(id)?].children.len())
    }

    fn children_in(
        &mut self,
        revision: u64,
        lens: SemanticLens,
        id: &str,
        request: PageRequest,
    ) -> Result<Page<EntitySummary>, BundleError> {
        self.check(revision)?;
        let parent = self.lookup(id)?;
        if !self.index.in_lens(lens, parent) {
            return Err(invalid("entity is outside the selected semantic lens"));
        }
        let query = format!("children:{lens:?}:{id}");
        let total = self.index.children_in(lens, parent).len();
        let mut offset = self.page_start(&query, &request, total)?;
        let mut items = Vec::new();
        let mut bytes = 0;
        while offset < total && items.len() < request.limit {
            let i = self.index.children_in(lens, parent)[offset].1 as usize;
            let len = self.index.entities[i].span.len;
            if bytes + len > self.limits.max_page_bytes {
                break;
            }
            items.push(self.entity(i)?.summary);
            bytes += len;
            offset += 1;
        }
        Ok(Page {
            revision,
            items,
            next: self.next(query, offset, total),
        })
    }

    fn children_at_in(
        &mut self,
        revision: u64,
        lens: SemanticLens,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Page<EntitySummary>, BundleError> {
        self.children_in(
            revision,
            lens,
            id,
            PageRequest {
                limit,
                after: Some(PageCursor {
                    model_id: self.manifest.model_id.clone(),
                    revision,
                    query: format!("children:{lens:?}:{id}"),
                    offset,
                }),
            },
        )
    }

    fn child_count_in(
        &self,
        revision: u64,
        lens: SemanticLens,
        id: &str,
    ) -> Result<usize, BundleError> {
        self.check(revision)?;
        let i = self.lookup(id)?;
        if !self.index.in_lens(lens, i) {
            return Err(invalid("entity is outside the selected semantic lens"));
        }
        Ok(self.index.children_in(lens, i).len())
    }

    fn region_relations(
        &mut self,
        revision: u64,
        request: RegionRequest,
        cancelled: &AtomicBool,
    ) -> Result<RegionPage, BundleError> {
        self.check(revision)?;
        if cancelled.load(Ordering::Relaxed) {
            return Err(BundleError::Cancelled);
        }
        if request.visible.is_empty()
            || request.visible.len() > 8448
            || request.scan_budget == 0
            || request.scan_budget > 8192
            || request.reference_budget > 250
            || request.existing_references.len() > request.reference_budget
            || request.scope_roots.len() > 128
        {
            return Err(BundleError::Limit("region query work budget"));
        }
        let focus = self.lookup(&request.focus)?;
        let lens = request.lens;
        if self.index.position_in(lens, focus).is_none() {
            return Err(invalid("focus is outside the selected semantic lens"));
        }
        let mut visible = request
            .visible
            .iter()
            .map(|id| self.lookup(id))
            .collect::<Result<Vec<_>, _>>()?;
        visible.sort_unstable();
        visible.dedup();
        if !visible.contains(&focus)
            || visible
                .iter()
                .any(|i| !self.index.contains_in(lens, focus, *i))
        {
            return Err(invalid(
                "region representatives must include focus and belong to its subtree",
            ));
        }
        let mut roots = if request.scope_roots.is_empty() {
            vec![focus]
        } else {
            request
                .scope_roots
                .iter()
                .map(|id| self.lookup(id))
                .collect::<Result<Vec<_>, _>>()?
        };
        roots.sort_unstable();
        roots.dedup();
        if roots
            .iter()
            .any(|root| !self.index.contains_in(lens, focus, *root))
        {
            return Err(invalid("region roots must belong to the supplied focus"));
        }
        let mut root_intervals: Vec<_> = roots
            .iter()
            .map(|i| {
                let position = self
                    .index
                    .position_in(lens, *i)
                    .expect("validated region root");
                (position.start, position.end)
            })
            .collect();
        root_intervals.sort_unstable();
        if root_intervals.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(invalid("region roots must be disjoint ownership subtrees"));
        }
        // A compact sweep of representative boundaries turns nearest-visible-
        // ancestor lookup into a binary search, including large local groups.
        let mut events = Vec::with_capacity(visible.len() * 2);
        for i in &visible {
            let position = self
                .index
                .position_in(lens, *i)
                .expect("validated representative");
            events.push((position.start, true, *i));
            events.push((position.end, false, *i));
        }
        events.sort_unstable();
        let mut active = std::collections::BTreeSet::new();
        let mut coverage = Vec::new();
        let mut event = 0;
        while event < events.len() {
            let position = events[event].0;
            while event < events.len() && events[event].0 == position {
                let (_, start, i) = events[event];
                let key = (
                    self.index
                        .position_in(lens, i)
                        .expect("validated representative")
                        .depth,
                    i,
                );
                if start {
                    active.insert(key);
                } else {
                    active.remove(&key);
                }
                event += 1;
            }
            coverage.push((position, active.last().map(|(_, i)| *i)));
        }
        let mut references = request
            .existing_references
            .iter()
            .map(|id| self.lookup(id))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
        let mut reference_limited = false;
        let legacy_query = format!("region:{lens:?}:{}:{visible:?}:{roots:?}", request.focus);
        // Persisted cursors from earlier versions retain their exact traversal.
        // Fresh queries use subtree incidence ranges, never a global scan.
        let legacy = request
            .page
            .after
            .as_ref()
            .is_some_and(|c| c.query == legacy_query);
        let query = if legacy {
            legacy_query
        } else {
            format!(
                "region-local-v1:{lens:?}:{}:{visible:?}:{roots:?}",
                request.focus
            )
        };
        let endpoints = self.index.region_endpoints(lens);
        let mut ranges = Vec::with_capacity(root_intervals.len());
        let mut local_total = 0;
        for (start, end) in &root_intervals {
            let first = endpoints.partition_point(|(p, _)| (*p as usize) < *start);
            let last = endpoints.partition_point(|(p, _)| (*p as usize) < *end);
            if first < last {
                ranges.push((local_total, first, last));
                local_total += last - first;
            }
        }
        let total = if legacy {
            self.index.relations.len()
        } else {
            local_total
        };
        let mut offset = self.page_start(&query, &request.page, total)?;
        let mut items = Vec::new();
        let mut internal_counts = std::collections::BTreeMap::new();
        let mut internal_relation_ids = Vec::new();
        let mut scanned = 0;
        let mut bytes = 0;
        while offset < total && scanned < request.scan_budget && items.len() < request.page.limit {
            if cancelled.load(Ordering::Relaxed) {
                return Err(BundleError::Cancelled);
            }
            let (endpoint_position, relation_index) = if legacy {
                (None, offset)
            } else {
                let range = ranges.partition_point(|(start, _, _)| *start <= offset) - 1;
                let (start, first, _) = ranges[range];
                let (position, relation) =
                    self.index.region_endpoints(lens)[first + offset - start];
                (Some(position as usize), relation as usize)
            };
            let indexed = &self.index.relations[relation_index];
            if !self.index.relation_in(lens, indexed) {
                scanned += 1;
                offset += 1;
                continue;
            }
            let structural = self.index.structural_in(lens, indexed);
            let source_index = indexed.source as usize;
            let target_index = indexed.target as usize;
            let in_scope = |i: usize| {
                let Some(position) = self.index.position_in(lens, i).map(|p| p.start) else {
                    return false;
                };
                root_intervals
                    .partition_point(|(start, _)| *start <= position)
                    .checked_sub(1)
                    .is_some_and(|j| position < root_intervals[j].1)
            };
            let source_inside = in_scope(source_index);
            let target_inside = in_scope(target_index);
            // Both endpoints of an internal edge occur in the index. Only its
            // source emits the original claim, including across separate roots.
            if !legacy
                && source_inside
                && target_inside
                && endpoint_position != self.index.position_in(lens, source_index).map(|p| p.start)
            {
                scanned += 1;
                offset += 1;
                continue;
            }
            let relevant = (source_inside || target_inside)
                && (!structural
                    || (visible.binary_search(&source_index).is_ok()
                        && visible.binary_search(&target_index).is_ok()));
            if relevant {
                let representative = |entity: usize, inside: bool| -> Result<usize, BundleError> {
                    let mapped = self
                        .index
                        .position_in(lens, entity)
                        .filter(|_| inside)
                        .and_then(|position| {
                            coverage
                                .partition_point(|(start, _)| *start <= position.start)
                                .checked_sub(1)
                                .and_then(|j| coverage[j].1)
                        });
                    if let Some(mapped) = mapped {
                        Ok(mapped)
                    } else {
                        // Ascend the external endpoint to the child of its common
                        // containing assembly with the focus. Keep its semantic home.
                        self.index.external_representative(lens, entity, focus, || {
                            if cancelled.load(Ordering::Relaxed) {
                                Err(BundleError::Cancelled)
                            } else {
                                Ok(())
                            }
                        })
                    }
                };
                let source = representative(source_index, source_inside)?;
                let target = representative(target_index, target_inside)?;
                if source == target && source_inside == target_inside {
                    *internal_counts
                        .entry(self.index.entities[source].id.to_string())
                        .or_insert(0) += 1;
                    if internal_relation_ids.len() < request.page.limit {
                        internal_relation_ids.push(indexed.id.to_string());
                    }
                } else {
                    let missing: std::collections::BTreeSet<_> =
                        [(source, source_inside), (target, target_inside)]
                            .into_iter()
                            .filter(|(i, inside)| {
                                (!inside || visible.binary_search(i).is_err())
                                    && !references.contains(i)
                            })
                            .map(|(i, _)| i)
                            .collect();
                    if references.len() + missing.len() > request.reference_budget {
                        reference_limited = true;
                        break;
                    }
                    references.extend(missing);
                    let span = indexed.span;
                    if bytes + span.len > self.limits.max_page_bytes {
                        break;
                    }
                    let source_display = self.index.entities[source].id.to_string();
                    let target_display = self.index.entities[target].id.to_string();
                    let BundleRecord::Relation(relation) = self.read(span)? else {
                        return Err(BundleError::SourceChanged);
                    };
                    items.push(ProjectedRelation {
                        relation,
                        structural,
                        source_display,
                        target_display,
                        source_inside,
                        target_inside,
                    });
                    bytes += span.len;
                }
            }
            scanned += 1;
            offset += 1;
        }
        Ok(RegionPage {
            revision,
            items,
            internal_counts,
            internal_relation_ids,
            scanned,
            reference_limited,
            next: self.next(query, offset, total),
        })
    }

    fn search(
        &mut self,
        revision: u64,
        text: &str,
        request: PageRequest,
        cancelled: &AtomicBool,
    ) -> Result<Page<EntitySummary>, BundleError> {
        self.check(revision)?;
        if cancelled.load(Ordering::Relaxed) {
            return Err(BundleError::Cancelled);
        }
        if text.is_empty() || text.len() > 256 {
            return Err(invalid("search requires 1–256 bytes"));
        }
        let text = text.to_lowercase();
        let query = format!("ranked-search:{text}");
        let count = self.index.entities.len();
        // Three disjoint index passes establish ranking without retaining every
        // match: exact identity/label, prefix, then substring. Each page still
        // examines at most 8,192 entries and loads only matching summaries.
        let total = count.saturating_mul(3);
        let mut offset = self.page_start(&query, &request, total)?;
        let stop = total.min(offset.saturating_add(8192));
        let mut items = Vec::new();
        let mut bytes = 0;
        while offset < stop && items.len() < request.limit {
            if cancelled.load(Ordering::Relaxed) {
                return Err(BundleError::Cancelled);
            }
            let index = offset % count;
            let phase = offset / count;
            let entity = &self.index.entities[index];
            let label = entity.label.to_lowercase();
            let exact = entity.id.as_ref() == text || label == text;
            let prefix = entity.id.starts_with(&text) || label.starts_with(&text);
            let matched = match phase {
                0 => exact,
                1 => !exact && prefix,
                _ => !prefix && (entity.id.contains(&text) || label.contains(&text)),
            };
            if matched {
                if bytes + entity.span.len > self.limits.max_page_bytes {
                    break;
                }
                bytes += entity.span.len;
                items.push(self.entity(index)?.summary);
            }
            offset += 1;
        }
        Ok(Page {
            revision,
            items,
            next: self.next(query, offset, total),
        })
    }

    fn detail_state(&self, revision: u64, id: &str) -> Result<DetailState, BundleError> {
        self.check(revision)?;
        let i = self.lookup(id)?;
        Ok(if !self.index.entities[i].has_detail {
            DetailState::NotSupplied
        } else if self.cache.contains_key(&i) {
            DetailState::Loaded
        } else {
            DetailState::Unloaded
        })
    }

    fn detail(&mut self, revision: u64, id: &str) -> Result<DetailResult, BundleError> {
        let state = self.detail_state(revision, id)?;
        let i = self.lookup(id)?;
        if state == DetailState::NotSupplied {
            return Ok(DetailResult {
                revision,
                state,
                detail: None,
            });
        }
        let detail = if let Some(entry) = self.cache.get(&i) {
            let detail = Arc::clone(&entry.detail);
            self.touch(i);
            detail
        } else {
            let detail = Arc::new(self.entity(i)?.detail.ok_or(BundleError::SourceChanged)?);
            self.retain_detail(i, Arc::clone(&detail));
            detail
        };
        // The returned record is loaded even if caching is disabled/too small;
        // a subsequent detail_state accurately reports provider residency.
        Ok(DetailResult {
            revision,
            state: DetailState::Loaded,
            detail: Some(detail),
        })
    }

    fn evidence(&mut self, revision: u64, id: &str) -> Result<Option<EvidenceRecord>, BundleError> {
        self.check(revision)?;
        let Some(Location::Evidence(span)) = self.index.ids.get(id) else {
            return Ok(None);
        };
        match self.read(*span)? {
            BundleRecord::Evidence(e) => Ok(Some(e)),
            _ => Err(BundleError::SourceChanged),
        }
    }
}

fn encode(record: &BundleRecord, limits: &Limits) -> Result<Vec<u8>, BundleError> {
    struct BoundedBuffer {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for BoundedBuffer {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            if data.len() > self.limit - self.bytes.len() {
                return Err(std::io::Error::other("record byte limit"));
            }
            self.bytes.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = BoundedBuffer {
        bytes: Vec::new(),
        limit: limits.max_record_bytes - 1,
    };
    if let Err(error) = serde_json::to_writer(&mut buffer, record) {
        return if error.is_io() {
            Err(BundleError::Limit("record bytes"))
        } else {
            Err(error.into())
        };
    }
    buffer.bytes.push(b'\n');
    Ok(buffer.bytes)
}

fn decode(bytes: &[u8]) -> Result<BundleRecord, BundleError> {
    // Bound nesting before Serde buffers an enum's content. Braces in labels or
    // escaped strings are text; invalid JSON syntax is still rejected by Serde.
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for &byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > 16 {
                        return Err(BundleError::Limit("JSON nesting depth"));
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(serde_json::from_slice(bytes)?)
}

fn read_line(
    reader: &mut impl BufRead,
    bytes: &mut Vec<u8>,
    max: usize,
) -> Result<bool, BundleError> {
    bytes.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(false)
            } else {
                Err(invalid("unterminated record"))
            };
        }
        let found = available.iter().position(|byte| *byte == b'\n');
        let consumed = found.map_or(available.len(), |i| i + 1);
        if consumed > max - bytes.len() {
            return Err(BundleError::Limit("record bytes"));
        }
        bytes.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if found.is_some() {
            return Ok(true);
        }
    }
}

fn checksum(bytes: &[u8]) -> u64 {
    // Change detection, not a cryptographic authenticity guarantee.
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn open_regular(path: &Path) -> Result<File, BundleError> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(invalid(
            "bundle input must name a regular file, not a link or directory",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    // Prevent a final-component symlink/FIFO substitution between metadata and
    // open on the supported macOS desktop and Linux test hosts. No extra crate.
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x0004 | 0x0100); // O_NONBLOCK | O_NOFOLLOW
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x0800 | 0x20000); // O_NONBLOCK | O_NOFOLLOW
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("bundle input must be a regular file"));
    }
    Ok(file)
}
