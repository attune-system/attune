# Protected file resource bounds

The v1 file codec is unchanged. It authenticates the complete binary file using
a base64/JSON/GCM envelope before disclosure. It is not a streaming AEAD codec.
The default plaintext upload cap remains 100 MiB. No smaller file-size cap or
new authorization denial is introduced by resource admission.

Each API instance has `artifacts.protected_file_memory_budget_bytes`, default
2 GiB, and `artifacts.file_transfer_concurrency`, default 4. Before allocating
codec buffers, protected publications and plaintext responses reserve
`12 * plaintext_bytes + 16 MiB`, rounded to MiB. This conservatively covers the
v1 codec's nested base64/JSON/AEAD buffers, Vec capacity growth, SQLx retry-body
transport and a retained plaintext response. It is a bound on admitted file
buffer allowances, not a claim about total process RSS. Deployment must leave
room for the API, database driver and other services, and measure real peak RSS.

Configuration validation requires the memory budget to cover the configured
maximum upload. Increasing the upload cap requires explicitly provisioning its
codec budget, not silently allocating based on a huge caller-supplied size.
Resource pressure returns a retryable resource error before file buffers are
allocated. An older historical file larger than the current budget needs a
sufficiently provisioned API instance; its original authority and bytes are not
changed. Ordinary streaming files reserve a fixed transport allowance rather
than a whole-file codec allowance. Plaintext response leases last until body
completion or drop.

S3 and GCS protected uploads now stream a single native conditional PutObject
request. S3 binds If-None-Match, length and SHA-256 checksum in the native
signature. GCS binds generation-match zero and length, then verifies actual
stored bytes. There is no additional whole-ciphertext collection in those native
adapters and no random remote multipart staging. Native single-request provider
size limits still apply, including S3's PutObject limit. Larger protected files
need a separately designed durable multipart protocol or streaming codec; this
implementation does not claim those are supported. Filesystem/test ObjectStore
adapters retain bounded payload fragments under the same instance allowance.

The instance tracks the entire publication task, including preparation and final
authorization. Dropping an HTTP response handle does not create an unowned task
or release its allowance. Shutdown closes admission, waits its configured grace,
then cancels and joins remaining local tasks. Cancellation can leave a remote
operation unresolved. It does not retire its durable operation or release pins;
native provider fencing remains mandatory. At most the configured concurrency
can run per instance, and the memory budget is shared across file readers and
writers. Startup/shutdown hosts and tests must drain `AppState.file_transfers`.

Static source only. Native conditional-header, resource-admission, caller-drop
and shutdown-join regressions are authored but unrun. Central provider, large-file
and process-memory acceptance is still required.
