# Ordinary file import

The client reads one explicit Android system document selection through its original read-only descriptor. The source must be a regular, seekable file from the platform-signed primary storage provider. Import makes one app-private full-body file, in 64 KiB chunks, without opening the source for write, interpreting the file format, or sending any request. The device must report enough usable bytes for the full body; a concurrent loss of space still aborts the copy. This stage does not impose the text editor size limit.

The caller allocates the operation ID before writing. A stage becomes usable after complete length and SHA-256 validation, a final source-grant and source-identity check, publication without overwriting another body, and a sync of its directory. The same ID and the exact length/digest recorded by the caller can recover an ambiguous publication. After verifying the exact body, recovery may remove its second pending hard link, not an unrelated name. A lone pending copy is not used or deleted at startup. Failed writes attempt cleanup only for the candidate inode they hold open.

This source step provides local staging; it does not connect the picker result to submit UI, persist a remote plan, or make arbitrary binary remote submissions available.
