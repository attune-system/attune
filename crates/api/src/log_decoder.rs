//! Bounded authenticated-frame delivery. EOF is successful only after F's final
//! manifest verifies the complete ciphertext, including its final newline.
use attune_common::runtime_log_completion::{FinalLogManifest, FrameVerifier};
use axum::body::Bytes;
use futures::{Stream, StreamExt, TryStreamExt};
use std::io;
use tokio::{sync::mpsc, task::JoinHandle};

type Ciphertext = std::pin::Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>;

/// Load segment metadata one exact sequence at a time, not an unbounded list.
pub(crate) async fn sealed_ciphertext(
    state: std::sync::Arc<crate::state::AppState>,
    version: &attune_common::models::ArtifactVersion,
) -> crate::middleware::ApiResult<Ciphertext> {
    use attune_common::{
        models::enums::LogStreamBackend, repositories::log_stream::LogStreamRepository,
    };
    let denied = || crate::middleware::ApiError::Forbidden("Protected log is unavailable".into());
    let stream = LogStreamRepository::find_by_artifact_version(&state.db, version.id)
        .await?
        .ok_or_else(denied)?;
    if !stream.sealed || stream.next_sequence < 0 || Some(stream.total_bytes) != version.size_bytes
    {
        return Err(denied());
    }
    if stream.backend == LogStreamBackend::SharedFile {
        return Ok(
            crate::routes::internal_files::stream_object_body(&state, version, None)
                .await
                .map_err(|_| denied())?
                .map_err(|_| invalid())
                .boxed(),
        );
    }
    let reader = futures::stream::try_unfold(
        (
            state,
            stream,
            0i64,
            0i64,
            None::<attune_common::blob_store::BlobReader>,
        ),
        |(state, stream, mut sequence, mut offset, mut current)| async move {
            loop {
                if let Some(reader) = &mut current {
                    if let Some(bytes) = reader.try_next().await.map_err(|_| invalid())? {
                        return Ok(Some((bytes, (state, stream, sequence, offset, current))));
                    }
                }
                if sequence == stream.next_sequence {
                    if offset != stream.total_bytes {
                        return Err(invalid());
                    }
                    return Ok(None);
                }
                let segment =
                    LogStreamRepository::find_segment_by_sequence(&state.db, stream.id, sequence)
                        .await
                        .map_err(|_| invalid())?
                        .ok_or_else(invalid)?;
                if segment.byte_start != offset
                    || segment.size_bytes < 0
                    || offset.checked_add(segment.size_bytes) != Some(segment.byte_end)
                {
                    return Err(invalid());
                }
                offset = segment.byte_end;
                sequence += 1;
                current = Some(
                    crate::routes::internal_files::stream_log_segments(
                        &state,
                        vec![segment],
                        None,
                        false,
                    )
                    .map_err(|_| invalid())?,
                );
            }
        },
    );
    Ok(reader.boxed())
}

fn invalid() -> io::Error {
    io::Error::other("Protected log is invalid or incomplete")
}

struct Delivery {
    receiver: mpsc::Receiver<Bytes>,
    producer: Option<JoinHandle<io::Result<()>>>,
}

pub(crate) enum LiveInput {
    Data(Bytes),
    Tick,
    Seal(FinalLogManifest),
}

/// Retain only an incomplete UTF-8 suffix between frames. Byte contents and
/// plaintext cursors stay unchanged, including invalid binary log bytes. Apply
/// the authority guard outside this adapter, after any wait for another frame.
pub(crate) fn text_boundaries<S>(reader: S) -> impl Stream<Item = io::Result<Bytes>>
where
    S: Stream<Item = io::Result<Bytes>>,
{
    futures::stream::try_unfold(
        (Box::pin(reader), Vec::<u8>::new(), false),
        |(mut reader, mut suffix, finished)| async move {
            if finished {
                return Ok(None);
            }
            let Some(bytes) = reader.try_next().await? else {
                if suffix.is_empty() {
                    return Ok(None);
                }
                return Ok(Some((Bytes::from(suffix), (reader, Vec::new(), true))));
            };
            if bytes.is_empty() {
                return Ok(Some((bytes, (reader, suffix, false))));
            }
            suffix.extend_from_slice(&bytes);
            let mut scan = 0;
            let end = loop {
                match std::str::from_utf8(&suffix[scan..]) {
                    Ok(_) => break suffix.len(),
                    Err(error) => {
                        scan += error.valid_up_to();
                        if let Some(length) = error.error_len() {
                            scan += length;
                        } else {
                            break scan;
                        }
                    }
                }
            };
            let pending = suffix.split_off(end);
            Ok(Some((Bytes::from(suffix), (reader, pending, false))))
        },
    )
}

/// Ciphertext offsets and object-segment sequences are independent of encrypted
/// frame sequences. Poll only bounded ranges/one segment, and never map idle or
/// process completion to successful EOF without the authenticated seal proof.
pub(crate) fn live_ciphertext(
    state: std::sync::Arc<crate::state::AppState>,
    pin: crate::live_logs::LogPin,
) -> impl Stream<Item = io::Result<LiveInput>> {
    use attune_common::{
        models::{enums::LogStreamBackend, ArtifactBodyState},
        repositories::{artifact::ArtifactVersionRepository, log_stream::LogStreamRepository},
        runtime_read_log::{open_final_manifest, ENCODING},
    };
    futures::stream::try_unfold(
        (
            state,
            pin,
            0i64,
            0u64,
            0u64,
            None::<attune_common::blob_store::BlobReader>,
            false,
        ),
        |(state, pin, mut next_segment, mut offset, mut end, mut reader, finished)| async move {
            if finished {
                return Ok(None);
            }
            loop {
                if let Some(current) = &mut reader {
                    if let Some(bytes) = current.try_next().await.map_err(|_| invalid())? {
                        offset = offset
                            .checked_add(bytes.len() as u64)
                            .filter(|offset| *offset <= end)
                            .ok_or_else(invalid)?;
                        return Ok(Some((
                            LiveInput::Data(bytes),
                            (state, pin, next_segment, offset, end, reader, false),
                        )));
                    }
                    if offset != end {
                        return Err(invalid());
                    }
                }
                let stream = LogStreamRepository::find_by_id_in_pool(&state.db, pin.stream)
                    .await
                    .map_err(|_| invalid())?;
                let version = ArtifactVersionRepository::find_by_id(&state.db, pin.version)
                    .await
                    .map_err(|_| invalid())?
                    .ok_or_else(invalid)?;
                if stream.artifact_version != pin.version
                    || stream.backend != pin.backend
                    || version.artifact != pin.artifact
                    || version.execution != Some(pin.execution)
                    || version.file_path != pin.file_path
                    || !matches!(
                        version.body_state,
                        Some(ArtifactBodyState::Pending | ArtifactBodyState::Ready)
                    )
                    || version
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.get("log_encoding"))
                        .and_then(serde_json::Value::as_str)
                        != Some(ENCODING)
                    || version
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.get("log_state"))
                        .and_then(serde_json::Value::as_str)
                        == Some("degraded")
                {
                    return Err(invalid());
                }
                if pin.backend == LogStreamBackend::SharedFile {
                    let snapshot = match crate::routes::internal_files::resolve_shared_file_log_snapshot(&state, &stream).await {
                        Ok(snapshot) => Some(snapshot),
                        Err(crate::routes::internal_files::LogStreamReadError::MissingSharedFile) if !stream.sealed && offset == 0 => None,
                        Err(_) => return Err(invalid()),
                    };
                    if let Some(snapshot) = snapshot {
                        if snapshot.size < offset {
                            return Err(invalid());
                        }
                        if snapshot.size > offset {
                            end = snapshot.size.min(offset.saturating_add(64 * 1024));
                            let range = attune_common::blob_store::ByteRange::new(offset, end)
                                .map_err(|_| invalid())?;
                            reader = Some(
                                crate::routes::internal_files::stream_shared_file_log(
                                    &state.config.artifacts_dir,
                                    stream.sealed,
                                    snapshot,
                                    Some(range),
                                )
                                .await
                                .map_err(|_| invalid())?,
                            );
                            continue;
                        }
                    }
                } else if let Some(segment) = LogStreamRepository::find_segment_by_sequence(
                    &state.db,
                    pin.stream,
                    next_segment,
                )
                .await
                .map_err(|_| invalid())?
                {
                    if segment.byte_start < 0
                        || segment.byte_start as u64 != offset
                        || segment.size_bytes <= 0
                        || segment.byte_start.checked_add(segment.size_bytes)
                            != Some(segment.byte_end)
                    {
                        return Err(invalid());
                    }
                    end = u64::try_from(segment.byte_end).map_err(|_| invalid())?;
                    next_segment = next_segment.checked_add(1).ok_or_else(invalid)?;
                    reader = Some(
                        crate::routes::internal_files::stream_log_segments(
                            &state,
                            vec![segment],
                            None,
                            false,
                        )
                        .map_err(|_| invalid())?,
                    );
                    continue;
                } else if next_segment != stream.next_sequence {
                    return Err(invalid());
                }
                if stream.sealed {
                    if version.body_state != Some(ArtifactBodyState::Ready)
                        || u64::try_from(stream.total_bytes).ok() != Some(offset)
                    {
                        return Err(invalid());
                    }
                    let key = state
                        .config
                        .security
                        .encryption_key
                        .as_deref()
                        .ok_or_else(invalid)?;
                    let proof = version
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.get("encrypted_log_manifest"))
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(invalid)?;
                    if proof.len() > attune_common::runtime_read_log::MAX_MANIFEST_BYTES {
                        return Err(invalid());
                    }
                    let manifest = open_final_manifest(key, proof, pin.execution, pin.version)
                        .map_err(|_| invalid())?;
                    if manifest.ciphertext_bytes != offset
                        || version.size_bytes != i64::try_from(offset).ok()
                        || version.sha256.as_deref() != Some(manifest.ciphertext_sha256.as_str())
                    {
                        return Err(invalid());
                    }
                    return Ok(Some((
                        LiveInput::Seal(manifest),
                        (state, pin, next_segment, offset, end, None, true),
                    )));
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                return Ok(Some((
                    LiveInput::Tick,
                    (state, pin, next_segment, offset, end, None, false),
                )));
            }
        },
    )
}

pub(crate) fn verified_live_plaintext<S>(
    reader: S,
    key: String,
    execution: i64,
    version: i64,
) -> impl Stream<Item = io::Result<Bytes>>
where
    S: Stream<Item = io::Result<LiveInput>> + Send + 'static,
{
    let (sender, receiver) = mpsc::channel(1);
    let producer = tokio::spawn(async move {
        use attune_common::runtime_read_log::{decode, FRAME_BYTES};
        // Durable producer permits bound publication. Reader memory is bounded
        // per frame regardless of a long-running stream's eventual total length.
        let mut verifier =
            FrameVerifier::new(&key, execution, version, i64::MAX as u64, i64::MAX as u64);
        let mut reader = Box::pin(reader);
        let mut pending = Vec::new();
        let mut sequence = 0u64;
        while let Some(input) = reader.try_next().await? {
            match input {
                LiveInput::Tick => {
                    if sender.send(Bytes::new()).await.is_err() {
                        return Ok(());
                    }
                }
                LiveInput::Seal(manifest) => {
                    if !pending.is_empty() {
                        return Err(invalid());
                    }
                    verifier.finish(&manifest).map_err(|_| invalid())?;
                    if reader.try_next().await?.is_some() {
                        return Err(invalid());
                    }
                    return Ok(());
                }
                LiveInput::Data(chunk) => {
                    for part in chunk.split_inclusive(|byte| *byte == b'\n') {
                        if pending
                            .len()
                            .checked_add(part.len())
                            .is_none_or(|size| size > FRAME_BYTES * 8)
                        {
                            return Err(invalid());
                        }
                        pending.extend_from_slice(part);
                        if pending.ends_with(b"\n") {
                            verifier.update(&pending).map_err(|_| invalid())?;
                            let plaintext = decode(&key, execution, version, sequence, &pending)
                                .map_err(|_| invalid())?;
                            sequence = sequence.checked_add(1).ok_or_else(invalid)?;
                            pending.clear();
                            if sender.send(Bytes::from(plaintext)).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
        Err(invalid())
    });
    futures::stream::try_unfold(
        Delivery {
            receiver,
            producer: Some(producer),
        },
        |mut delivery| async move {
            if let Some(bytes) = delivery.receiver.recv().await {
                return Ok(Some((bytes, delivery)));
            }
            if let Some(producer) = delivery.producer.take() {
                producer.await.map_err(|_| invalid())??;
            }
            Ok(None)
        },
    )
}

impl Drop for Delivery {
    fn drop(&mut self) {
        if let Some(producer) = &self.producer {
            producer.abort();
        }
    }
}

/// At most one queued plaintext frame plus one in production. The async owner
/// keeps F's borrowed-key verifier alive without leaking keys or self-references.
/// Authorization belongs AFTER this stream's queue wait, in the delivery guard.
pub(crate) fn verified_plaintext<S, E>(
    reader: S,
    key: String,
    expected: FinalLogManifest,
) -> impl Stream<Item = io::Result<Bytes>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let (sender, receiver) = mpsc::channel(1);
    let producer = tokio::spawn(async move {
        use attune_common::runtime_read_log::{decode, FRAME_BYTES};
        let mut verifier = FrameVerifier::for_manifest(&key, &expected);
        let mut reader = Box::pin(reader);
        let mut pending = Vec::new();
        let mut sequence = 0u64;
        while let Some(chunk) = reader.try_next().await.map_err(|_| invalid())? {
            for part in chunk.split_inclusive(|byte| *byte == b'\n') {
                if pending
                    .len()
                    .checked_add(part.len())
                    .is_none_or(|size| size > FRAME_BYTES * 8)
                {
                    return Err(invalid());
                }
                pending.extend_from_slice(part);
                if !pending.ends_with(b"\n") {
                    continue;
                }
                verifier.update(&pending).map_err(|_| invalid())?;
                let plaintext = decode(
                    &key,
                    expected.execution,
                    expected.version,
                    sequence,
                    &pending,
                )
                .map_err(|_| invalid())?;
                sequence = sequence.checked_add(1).ok_or_else(invalid)?;
                pending.clear();
                if sender.send(Bytes::from(plaintext)).await.is_err() {
                    return Ok(());
                }
            }
        }
        if !pending.is_empty() {
            return Err(invalid());
        }
        verifier.finish(&expected).map_err(|_| invalid())
    });
    futures::stream::try_unfold(
        Delivery {
            receiver,
            producer: Some(producer),
        },
        |mut delivery| async move {
            if let Some(chunk) = delivery.receiver.recv().await {
                return Ok(Some((chunk, delivery)));
            }
            if let Some(producer) = delivery.producer.take() {
                producer.await.map_err(|_| invalid())??;
            }
            Ok(None)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::{
        runtime_log_completion::FrameTotals,
        runtime_read_log::{encode, FRAME_BYTES, MAX_FINITE_PLAINTEXT},
    };
    use futures::StreamExt;
    const KEY: &str = "bounded-log-decoder-key-at-least-32-bytes";

    #[tokio::test]
    async fn sealed_short_and_empty_split_frames_complete() {
        for plaintext in [
            [b"first".as_slice(), b"last".as_slice(), b"".as_slice()],
            [b"".as_slice(), b"".as_slice(), b"".as_slice()],
        ] {
            let mut totals = FrameTotals::default();
            let mut ciphertext = Vec::new();
            for (sequence, bytes) in plaintext.iter().enumerate() {
                let frame = encode(KEY, 11, 12, sequence as u64, bytes).unwrap();
                totals.record(&frame, bytes.len()).unwrap();
                ciphertext.extend(frame);
            }
            let expected = totals.manifest(11, 12).unwrap();
            let expected =
                FinalLogManifest::open(KEY, &expected.authenticate(KEY).unwrap()).unwrap();
            assert_eq!(expected.frame_count, 3);
            assert_eq!(expected.plaintext_bytes, plaintext.concat().len() as u64);
            assert_eq!(expected.ciphertext_bytes, ciphertext.len() as u64);
            let chunks = ciphertext
                .chunks(7)
                .map(|chunk| Ok::<_, io::Error>(Bytes::copy_from_slice(chunk)))
                .collect::<Vec<_>>();
            let stream = verified_plaintext(futures::stream::iter(chunks), KEY.into(), expected);
            futures::pin_mut!(stream);
            let mut decoded = Vec::new();
            for bytes in plaintext {
                let frame = stream
                    .try_next()
                    .await
                    .unwrap()
                    .expect("complete authenticated frame");
                assert_eq!(frame.as_ref(), bytes);
                decoded.extend_from_slice(&frame);
            }
            assert_eq!(decoded, plaintext.concat());
            assert!(
                stream.try_next().await.unwrap().is_none(),
                "verified successful EOF"
            );
            assert!(stream.next().await.is_none());
        }
    }

    #[tokio::test]
    async fn sealed_split_frames_reject_extra_frames_and_wrong_manifest_boundaries() {
        let mut totals = FrameTotals::default();
        let mut ciphertext = Vec::new();
        for (sequence, bytes) in [b"first".as_slice(), b"last".as_slice(), b"".as_slice()]
            .iter()
            .enumerate()
        {
            let frame = encode(KEY, 11, 12, sequence as u64, bytes).unwrap();
            totals.record(&frame, bytes.len()).unwrap();
            ciphertext.extend(frame);
        }
        let expected = totals.manifest(11, 12).unwrap();
        let extra = encode(KEY, 11, 12, expected.frame_count, b"").unwrap();
        let extended = [ciphertext.clone(), extra.clone()].concat();
        let mut wrong_hash = expected.clone();
        wrong_hash.ciphertext_sha256.replace_range(
            ..1,
            if expected.ciphertext_sha256.starts_with('0') {
                "1"
            } else {
                "0"
            },
        );
        for (label, manifest, bytes) in [
            ("extra frame", expected.clone(), extended.clone()),
            (
                "frame cap independent of ciphertext cap",
                FinalLogManifest {
                    ciphertext_bytes: extended.len() as u64,
                    ..expected.clone()
                },
                extended.clone(),
            ),
            (
                "hash after permitted extra empty frame",
                FinalLogManifest {
                    frame_count: expected.frame_count + 1,
                    ciphertext_bytes: extended.len() as u64,
                    ..expected.clone()
                },
                extended,
            ),
            (
                "reduced frame count",
                FinalLogManifest {
                    frame_count: 2,
                    ..expected.clone()
                },
                ciphertext.clone(),
            ),
            (
                "reduced plaintext cap",
                FinalLogManifest {
                    plaintext_bytes: expected.plaintext_bytes - 1,
                    ..expected.clone()
                },
                ciphertext.clone(),
            ),
            (
                "missing plaintext byte",
                FinalLogManifest {
                    plaintext_bytes: expected.plaintext_bytes + 1,
                    ..expected.clone()
                },
                ciphertext.clone(),
            ),
            (
                "reduced ciphertext cap",
                FinalLogManifest {
                    ciphertext_bytes: expected.ciphertext_bytes - 1,
                    ..expected.clone()
                },
                ciphertext.clone(),
            ),
            (
                "missing ciphertext byte",
                FinalLogManifest {
                    ciphertext_bytes: expected.ciphertext_bytes + 1,
                    ..expected.clone()
                },
                ciphertext.clone(),
            ),
            ("wrong ciphertext hash", wrong_hash, ciphertext.clone()),
        ] {
            let manifest =
                FinalLogManifest::open(KEY, &manifest.authenticate(KEY).unwrap()).unwrap();
            let chunks = bytes
                .chunks(7)
                .map(|chunk| Ok::<_, io::Error>(Bytes::copy_from_slice(chunk)))
                .collect::<Vec<_>>();
            let stream = verified_plaintext(futures::stream::iter(chunks), KEY.into(), manifest);
            futures::pin_mut!(stream);
            let mut delivered = 0;
            loop {
                match stream.try_next().await {
                    Ok(Some(_)) => delivered += 1,
                    Ok(None) => panic!("{label} must not produce successful EOF"),
                    Err(_) => break,
                }
            }
            let expected_delivered = match label {
                "reduced frame count" | "reduced ciphertext cap" => 2,
                "reduced plaintext cap" => 1,
                "hash after permitted extra empty frame" => 4,
                _ => 3,
            };
            assert_eq!(
                delivered, expected_delivered,
                "wrong rejection boundary: {label}"
            );
            assert!(
                stream.next().await.is_none(),
                "error terminates delivery: {label}"
            );
        }
    }

    #[tokio::test]
    async fn sealed_zero_frame_manifest_rejects_nonempty_ciphertext() {
        let empty = FrameTotals::default().manifest(11, 12).unwrap();
        let empty = FinalLogManifest::open(KEY, &empty.authenticate(KEY).unwrap()).unwrap();
        assert_eq!(empty.frame_count, 0);
        assert_eq!(empty.plaintext_bytes, 0);
        assert_eq!(empty.ciphertext_bytes, 0);
        let ciphertext = encode(KEY, 11, 12, 0, b"").unwrap();
        let chunks = ciphertext
            .chunks(7)
            .map(|chunk| Ok::<_, io::Error>(Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>();
        let stream = verified_plaintext(futures::stream::iter(chunks), KEY.into(), empty);
        futures::pin_mut!(stream);
        assert!(stream.try_next().await.is_err());
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn sealed_log_larger_than_the_old_finite_cap_completes_with_bounded_frames() {
        let mut totals = FrameTotals::default();
        let frames = (0..MAX_FINITE_PLAINTEXT / FRAME_BYTES as u64 + 1)
            .map(|sequence| {
                let frame = encode(KEY, 11, 12, sequence, &vec![0; FRAME_BYTES]).unwrap();
                totals.record(&frame, FRAME_BYTES).unwrap();
                Ok::<_, io::Error>(Bytes::from(frame))
            })
            .collect::<Vec<_>>();
        let expected = totals.manifest(11, 12).unwrap();
        let expected_bytes = expected.plaintext_bytes;
        let stream = verified_plaintext(futures::stream::iter(frames), KEY.into(), expected);
        futures::pin_mut!(stream);
        let mut bytes = 0u64;
        while let Some(frame) = stream.try_next().await.unwrap() {
            assert!(frame.len() <= FRAME_BYTES);
            bytes += frame.len() as u64;
        }
        assert_eq!(bytes, expected_bytes);
        assert!(bytes > MAX_FINITE_PLAINTEXT);
    }

    #[tokio::test]
    async fn valid_prefix_is_not_successful_eof_and_corrupt_final_frame_is_not_done() {
        let first = encode(KEY, 11, 12, 0, b"first").unwrap();
        let last = encode(KEY, 11, 12, 1, b"last").unwrap();
        let mut totals = FrameTotals::default();
        totals.record(&first, 5).unwrap();
        totals.record(&last, 4).unwrap();
        let expected = totals.manifest(11, 12).unwrap();
        for bytes in [
            first.clone(),
            [first.clone(), last[..last.len() - 1].to_vec()].concat(),
        ] {
            let chunks = bytes
                .chunks(3)
                .map(|chunk| Ok::<_, io::Error>(Bytes::copy_from_slice(chunk)))
                .collect::<Vec<_>>();
            let stream =
                verified_plaintext(futures::stream::iter(chunks), KEY.into(), expected.clone());
            futures::pin_mut!(stream);
            assert_eq!(stream.try_next().await.unwrap().unwrap().as_ref(), b"first");
            assert!(stream.try_next().await.is_err());
            assert!(stream.next().await.is_none());
        }
        let empty = FrameTotals::default().manifest(11, 12).unwrap();
        let stream = verified_plaintext(
            futures::stream::empty::<io::Result<Bytes>>(),
            KEY.into(),
            empty,
        );
        futures::pin_mut!(stream);
        assert!(stream.try_next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn live_decoder_keeps_partial_frames_across_idle_ticks_and_requires_final_proof() {
        let frame = encode(KEY, 11, 12, 0, b"live-frame").unwrap();
        let mut totals = FrameTotals::default();
        totals.record(&frame, 10).unwrap();
        let manifest = totals.manifest(11, 12).unwrap();
        let split = frame.len() / 2;
        let inputs = vec![
            Ok(LiveInput::Data(Bytes::copy_from_slice(&frame[..split]))),
            Ok(LiveInput::Tick),
            Ok(LiveInput::Data(Bytes::copy_from_slice(&frame[split..]))),
            Ok(LiveInput::Seal(manifest)),
        ];
        let stream = verified_live_plaintext(futures::stream::iter(inputs), KEY.into(), 11, 12);
        futures::pin_mut!(stream);
        assert!(stream.try_next().await.unwrap().unwrap().is_empty());
        assert_eq!(
            stream.try_next().await.unwrap().unwrap().as_ref(),
            b"live-frame"
        );
        assert!(stream.try_next().await.unwrap().is_none());

        let stream = verified_live_plaintext(
            futures::stream::iter(vec![Ok(LiveInput::Data(Bytes::from(frame)))]),
            KEY.into(),
            11,
            12,
        );
        futures::pin_mut!(stream);
        assert_eq!(
            stream.try_next().await.unwrap().unwrap().as_ref(),
            b"live-frame"
        );
        assert!(
            stream.try_next().await.is_err(),
            "valid prefix without seal is not EOF"
        );
    }

    #[tokio::test]
    async fn live_decoder_rejects_unbounded_unterminated_frame_and_spliced_manifest() {
        let stream = verified_live_plaintext(
            futures::stream::iter(vec![Ok(LiveInput::Data(Bytes::from(vec![
                b'x';
                attune_common::runtime_read_log::FRAME_BYTES
                    * 8
                    + 1
            ])))]),
            KEY.into(),
            11,
            12,
        );
        futures::pin_mut!(stream);
        assert!(stream.try_next().await.is_err());

        let manifest = FrameTotals::default().manifest(11, 13).unwrap();
        let stream = verified_live_plaintext(
            futures::stream::iter(vec![Ok(LiveInput::Seal(manifest))]),
            KEY.into(),
            11,
            12,
        );
        futures::pin_mut!(stream);
        assert!(stream.try_next().await.is_err());
    }

    #[tokio::test]
    async fn live_text_boundaries_preserve_cross_frame_codepoints_idle_ticks_and_binary_bytes() {
        let parts = vec![
            Ok(Bytes::from_static(&[b'a', 0xf0, 0x9f])),
            Ok(Bytes::new()),
            Ok(Bytes::from_static(&[0x98, 0x80, 0xff, b'z'])),
        ];
        let stream = text_boundaries(futures::stream::iter(parts));
        futures::pin_mut!(stream);
        assert_eq!(stream.try_next().await.unwrap().unwrap().as_ref(), b"a");
        assert!(stream.try_next().await.unwrap().unwrap().is_empty());
        assert_eq!(
            stream.try_next().await.unwrap().unwrap().as_ref(),
            &[0xf0, 0x9f, 0x98, 0x80, 0xff, b'z']
        );
        assert!(stream.try_next().await.unwrap().is_none());
    }
}
