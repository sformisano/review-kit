# Quantity Store

This library stores text and represents positive item quantities.

`Quantity` always contains a value greater than zero. JSON serialization uses an
object with one numeric `value` field. Supported JSON input keeps that format and
the same positive-value guarantee.

`save` replaces the primary text file. A `SaveError::Retryable` result means no
primary data was saved, so the caller can retry. Audit output is optional: an
unavailable audit destination must not turn a committed primary save into a
retryable failure.

Run the ordinary offline checks with `cargo test`. Tests create their own
temporary directory under `TMPDIR`, falling back to the system temporary directory.
