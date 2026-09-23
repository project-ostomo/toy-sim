# Protocol 49 conformance examples

[protocol-vectors.json](protocol-vectors.json) contains hexadecimal bytes and
their input values. Hex strings contain no whitespace; decode each pair of digits
as one byte. The file has these sections:

| Section | Contents |
| --- | --- |
| `handshake` | Fixed example keys, hellos, all handshake digests/signatures, directional encryption keys and encrypted records |
| `messages` | Six framed Session/Input/State examples, including an empty state and a subscription |
| `primitives` | Integer boundaries, options, Unicode, negative zero and a wide galactic coordinate |
| `codec_fixtures` | 262 examples covering every declaration and every enum variant in the complete payload schema |

`main_stream_hex` includes the four-byte length prefix. `postcard_hex` is only
the named type's Postcard encoding: add a Message wrapper and length prefix only
when that type is used through the main stream. Upload acknowledgements and
inhabited-directory assets have neither wrapper nor prefix.

The `messages.value` objects describe values using JSON notation; JSON is not
sent over the main stream. `codec_fixtures.value_rust` gives an exact constructor
expression using the names from [the schema](protocol-schema.md). Map/set
constructors explicitly list entries so that non-string keys are unambiguous.
The `.into()` on strings means an owned UTF-8 string. These fixtures exercise
serialization; their arbitrary numbers and identifiers do not imply authorized
or semantically valid game commands. The six main-message examples satisfy
structural input validation; successful actions additionally require the session
state and authority described in [the protocol](protocol.md).

## Primitive checks

| Type and value | Postcard bytes |
| --- | --- |
| `u64` 128 | `80 01` |
| `i64` −65 | `81 01` |
| `Option<u64>::None` | `00` |
| `Option<u64>::Some(128)` | `01 80 01` |
| `char` λ | `02 ce bb` |
| `f64` −0.0 | `00 00 00 00 00 00 00 80` |

The wide-position example includes `z = 2^110` micrometres. It detects encoders
that accidentally narrow coordinates to 64 bits or encode them as fixed-width
integers. The extreme i128 examples test the primitive codec; values outside the
position limit are invalid in client-supplied positions.

## Main-stream example

An empty Input for world `01010101-0101-0101-0101-010101010101`, with sequence 0:

```text
00 00 00 13                         body length: 19 (big-endian)
01                                  Message::Input
01 01 01 01 01 01 01 01             world ID, first eight bytes
01 01 01 01 01 01 01 01             world ID, last eight bytes
00                                  sequence: 0
00                                  actions count: 0
```

The subscription example uses command ID `[4;16]`, view ID/revision 1, and focused
ship `[2;16]`. Its Action discriminant is **13**, the declaration index of
Subscribe. Its Input has sequence 1. The throttle example has sequence 2 and
sets throttle to 0.5 with authority revision 1. The chat example uses sequence 3
and subscription revision 1; a server requires an existing chat subscription.
These examples are independently usable messages, rather than a complete gameplay
script. The empty State deliberately contains no ship, view, events or results.

## Deterministic handshake

The vectors use the following fixed test inputs. Production handshakes generate
fresh random ephemeral private keys for every connection.

| Input | Value |
| --- | --- |
| Client X25519 scalar input | Byte `31` repeated 32 times, clamped by X25519 |
| Server X25519 scalar input | Byte `42` repeated 32 times, clamped by X25519 |
| Server Ed25519 seed | Byte `55` repeated 32 times |
| Account Ed25519 seed | Byte `66` repeated 32 times |
| Account ID | Byte `77` repeated 16 times |

Each hello is 34 bytes: version prefix `00 31` followed by the 32-byte ephemeral
public key. With the exact context strings from the
specification, the transcript digest is:

```text
26fd742cc69a004239488a067b600866f6668d67c5554bce958d02ae71538cf3
```

The client-to-server encryption key is:

```text
4fcbf3f237875fc7793b6d09b6dffad8258fd763d6ff2b6bd2983e667a877c49
```

The server-to-client encryption key is:

```text
215f24dbbea2ac3cdadc57a95cf9e94aff8d986d10ad5dc81014d36b6323b476
```

Exchange `client_hello`, then `server_hello || server_signature`, then
`client_auth_record`, then `server_authenticated_record`. The two authentication
records use sequence 0 in their respective directions. All encrypted-record
examples use empty associated data and a 96-bit big-endian sequence nonce.
For sequence 1 that nonce is `00 00 00 00 00 00 00 00 00 00 00 01`.

`client_sequence_1_record` carries the provided `compressed_bytes_hex`, which
decompresses to `compression_plaintext_hex` (ASCII `transport sample`). This is
an isolated compression/record example. In an application connection, feed
picomux's output into compression and encryption.

A compatible Zstandard encoder can produce different compressed bytes; compare
the decompressed bytes with the sample. Feed the supplied compressed bytes when
checking the exact encrypted sequence-1 record. Picomux itself is supplied by
the library and has no independent wire examples in this specification.

The sequence-4,294,967,296 example tests the upper half of the 64-bit sequence
nonce using the same directional key. Its data is the ASCII string `later-record`
solely to test the record codec; it is not compressed
application traffic. The server sequence-2 close is likewise an isolated record
example. Counters must advance through all intervening records in a real session;
these isolated vectors do not authorize sequence jumps.

## Checking an independent Rust schema

Copy `protocol-schema.rs` and `protocol-vectors.json` into a separate directory.
This manifest builds the schema with no game or engine dependencies:

```toml
[package]
name = "protocol-schema"
version = "0.0.0"
edition = "2024"

[lib]
path = "protocol-schema.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }

[dev-dependencies]
postcard = { version = "=1.1.3", features = ["alloc"] }
serde_json = "1"
```

Place this in `tests/vectors.rs` and run `cargo test`. It checks framed values,
complete consumption and encoding of the six application examples:

```rust
use protocol_schema::Message;

#[test]
fn application_vectors() {
    let corpus: serde_json::Value = serde_json::from_str(
        include_str!("../protocol-vectors.json"),
    ).unwrap();

    for example in corpus["messages"].as_array().unwrap() {
        let hex = example["main_stream_hex"].as_str().unwrap();
        let wire: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let length = u32::from_be_bytes(wire[..4].try_into().unwrap()) as usize;
        assert_eq!(length, wire.len() - 4);

        let expected: Message = serde_json::from_value(example["value"].clone()).unwrap();
        let (received, remaining) = postcard::take_from_bytes::<Message>(&wire[4..]).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(received, expected);
        assert_eq!(postcard::to_allocvec(&received).unwrap(), wire[4..]);
    }
}
```

For each codec fixture, instantiate its value using the schema, serialize it and
compare `postcard_hex`. Decode those bytes back into the named type and require
complete consumption. The fixture set was checked field by field against the
version-49 message types, including every enum variant, with nonempty nested
collections and present optional values. Separate application/primitive examples
cover empty collections and absent options. The handshake was exercised with an
independent peer in each direction, and the encrypted vectors were compared with
the version-49 record encoder and decoder.

## Failure and lifecycle checks

An implementation should also exercise the observable failures defined by the
protocol:

| Stimulus | Required result |
| --- | --- |
| Hello starts with version 48 | Authentication ends without establishing a session |
| Flip a bit in an authenticated record | AEAD verification fails; connection ends |
| Change a record length and truncate or extend its ciphertext accordingly | AEAD verification fails; connection ends |
| Truncate a record or close TCP without the close record | Transport failure |
| Main body contains only enum tag 3 | Unknown Message variant; connection ends |
| Main length header exceeds 8,388,608 | Reject before allocating/reading that body |
| Input uses screen slot 8 | Input validation fails; connection ends |
| Duplicate command IDs within one Input | Input validation fails; connection ends |
| Valid Input names another world | Ignore actions and preserve sequence history |
| Input repeats a current-world sequence | Session ends |
| New sequence repeats an earlier command ID | Skip action without repeating its result |
| New command ID has stale authority revision | CommandResult error; session continues |
| Receive new Session world | Clear session observations/subscriptions and establish new input history |
| Write shutdown after asset request or complete upload | Opposite direction remains usable for response until EOF |
| Industry/chat absent while skipping a render snapshot | Preserve earlier updates and all transient results/events |

These cases describe the protocol contract. They do not require any particular
threading model, game engine, scene graph or client playback algorithm.
