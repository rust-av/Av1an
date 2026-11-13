# condor.output

`condor.output` in `condor.json`. See [condor](./index.md), [top level](../index.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `path` | String (path) | Yes | — | Output video path. Extension must be supported by the [concatenation method](../../types/concatenation.md) (`mkvmerge`/`ffmpeg` → `.mkv`, `ivf` → `.ivf`) |
| `overwrite` | Boolean | No | `false` | Explicit saved consent to replace an existing distinct final output after successful muxing |
| `tags` | Object (string→string) | Yes | `{}` | Container tags |
| `video_tags` | Object (string→string) | Yes | `{}` | Video stream tags |

Example:

```json
{
    "path": "output.mkv",
    "overwrite": false,
    "tags": {},
    "video_tags": {}
}
```

Existing outputs are refused by default, including unattended runs; there is no prompt.
`av1an --overwrite`, `av1an init INPUT OUTPUT --overwrite`, and
`av1an concatenate --overwrite` save consent. Omitting the flag keeps saved consent.
Revoke it with `av1an init set overwrite false` or `av1an init reset overwrite`.
Older JSON without this field defaults to `false`.

The output cannot alias any declared root/stage input, mux input, attachment source,
or encoded chunk, even when a source's tracks are disabled or overwrite is enabled.
Checks resolve symlinks and existing-file identity (including hardlinks on Unix and
Windows). Symbolic-link output leaves, directories and special files are refused.
Paths hidden inside inline scripts cannot be inferred.

Offline output edits may store a missing parent, but execution requires an existing
directory and does not create it. Final muxing uses owned sibling staging; failed
muxes and forced cancellation do not replace the old final output. Identity and
existence are checked again before publication, with atomic no-clobber publication
when consent is false.

With tags:

```json
{
    "path": "output.mkv",
    "tags": { "title": "My Encode" },
    "video_tags": { "language": "eng" }
}
```
