# Clean

Cleanup is unavailable. Both forms return a nonzero error without removing files
or loading the configuration. Ordinary logging may still create or update the log.

```bash
av1an clean
av1an clean --all
```

| Name                      | Flag    | Type | Default |
| ------------------------- | ------- | ---- | ------- |
| [All](#all---all)         | `--all` |      |         |

## All `--all`

Reserved; not implemented. This flag does not remove any files.

## Start a separate job

Use a different `--config-file`, a distinct empty `--temp` directory, and a distinct
output so the new job cannot reuse stale chunks or replace the old output. Choose
unused paths for this example:

```bash
av1an --config-file new-job.json --temp new-job-work init input.mp4 new-output.mkv
```

This creates a separate configuration; it does not clean or reset the old job.
Resetting saved scene/measurement metadata does not remove physical encode or probe
files. See [Editing saved settings](config-editing.md) and [Scenes](scenes.md).

## Screenshots

These screenshots may show older help; the text above describes current behavior.
Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-clean-light.avif"><picture><source srcset="../media/tui/help-clean-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-clean-light.avif" alt="av1an clean --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an clean --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-clean-verbose-light.avif"><picture><source srcset="../media/tui/help-clean-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-clean-verbose-light.avif" alt="av1an clean --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an clean --help --verbose</p>
    </td>
  </tr>
</table>
