# Screenshots

Every image on this page is **generated from the live interface** by an
integration test (`cargo test --features screenshots`) and the
`gen_screenshots` binary, then encoded as 1920x1080 AVIF. Because they are
rendered through the exact code the terminal uses, they can never fall out of
date: any change to a screen is reflected the next time they are generated.

The light and dark variants are chosen automatically by your system preference
via a `prefers-color-scheme` media query. Click any image to open it full size.

The most relevant screenshots are also embedded in each command's own page —
follow the [Commands](commands/condor.md) section to see them in context.

## Interface

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/scene-detection-light.avif"><picture><source srcset="media/tui/scene-detection-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/scene-detection-light.avif" alt="Detecting scenes" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Detecting scenes</p>
    </td>
    <td width="50%" valign="top"></td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/noise-detection-light.avif"><picture><source srcset="media/tui/noise-detection-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/noise-detection-light.avif" alt="Detecting noise" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Detecting noise</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/benchmarker-light.avif"><picture><source srcset="media/tui/benchmarker-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/benchmarker-light.avif" alt="Benchmarking workers" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Benchmarking workers</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/target-quality-encoding-light.avif"><picture><source srcset="media/tui/target-quality-encoding-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/target-quality-encoding-light.avif" alt="Target Quality — encoding" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Target Quality — encoding</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/target-quality-comparing-light.avif"><picture><source srcset="media/tui/target-quality-comparing-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/target-quality-comparing-light.avif" alt="Target Quality — comparing" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Target Quality — comparing</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/parallel-encoder-light.avif"><picture><source srcset="media/tui/parallel-encoder-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/parallel-encoder-light.avif" alt="Parallel encoder" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Parallel encoder</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/scene-concatenator-light.avif"><picture><source srcset="media/tui/scene-concatenator-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/scene-concatenator-light.avif" alt="Scene concatenator" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Scene concatenator</p>
    </td>
  </tr>
</table>

## Command Help

`av1an --help` and the per-command help, in both the short and verbose
(`--help --verbose`) forms.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-light.avif"><picture><source srcset="media/tui/help-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-light.avif" alt="av1an --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-verbose-light.avif"><picture><source srcset="media/tui/help-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-verbose-light.avif" alt="av1an --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-init-light.avif"><picture><source srcset="media/tui/help-init-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-init-light.avif" alt="av1an init --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an init --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-init-verbose-light.avif"><picture><source srcset="media/tui/help-init-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-init-verbose-light.avif" alt="av1an init --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an init --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-detect-scenes-light.avif"><picture><source srcset="media/tui/help-detect-scenes-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-detect-scenes-light.avif" alt="av1an detect-scenes --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-scenes --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-detect-scenes-verbose-light.avif"><picture><source srcset="media/tui/help-detect-scenes-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-detect-scenes-verbose-light.avif" alt="av1an detect-scenes --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-scenes --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-zone-light.avif"><picture><source srcset="media/tui/help-zone-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-zone-light.avif" alt="av1an zone --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an zone --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-zone-verbose-light.avif"><picture><source srcset="media/tui/help-zone-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-zone-verbose-light.avif" alt="av1an zone --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an zone --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-detect-noise-light.avif"><picture><source srcset="media/tui/help-detect-noise-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-detect-noise-light.avif" alt="av1an detect-noise --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-noise --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-detect-noise-verbose-light.avif"><picture><source srcset="media/tui/help-detect-noise-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-detect-noise-verbose-light.avif" alt="av1an detect-noise --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-noise --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-scale-noise-light.avif"><picture><source srcset="media/tui/help-scale-noise-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-scale-noise-light.avif" alt="av1an scale-noise --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an scale-noise --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-scale-noise-verbose-light.avif"><picture><source srcset="media/tui/help-scale-noise-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-scale-noise-verbose-light.avif" alt="av1an scale-noise --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an scale-noise --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-benchmark-light.avif"><picture><source srcset="media/tui/help-benchmark-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-benchmark-light.avif" alt="av1an benchmark --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an benchmark --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-benchmark-verbose-light.avif"><picture><source srcset="media/tui/help-benchmark-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-benchmark-verbose-light.avif" alt="av1an benchmark --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an benchmark --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-target-quality-light.avif"><picture><source srcset="media/tui/help-target-quality-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-target-quality-light.avif" alt="av1an target-quality --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an target-quality --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-target-quality-verbose-light.avif"><picture><source srcset="media/tui/help-target-quality-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-target-quality-verbose-light.avif" alt="av1an target-quality --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an target-quality --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-optimize-bitrate-light.avif"><picture><source srcset="media/tui/help-optimize-bitrate-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-optimize-bitrate-light.avif" alt="av1an optimize-bitrate --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an optimize-bitrate --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-optimize-bitrate-verbose-light.avif"><picture><source srcset="media/tui/help-optimize-bitrate-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-optimize-bitrate-verbose-light.avif" alt="av1an optimize-bitrate --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an optimize-bitrate --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-scale-speed-light.avif"><picture><source srcset="media/tui/help-scale-speed-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-scale-speed-light.avif" alt="av1an scale-speed --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an scale-speed --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-scale-speed-verbose-light.avif"><picture><source srcset="media/tui/help-scale-speed-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-scale-speed-verbose-light.avif" alt="av1an scale-speed --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an scale-speed --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-encode-light.avif"><picture><source srcset="media/tui/help-encode-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-encode-light.avif" alt="av1an encode --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an encode --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-encode-verbose-light.avif"><picture><source srcset="media/tui/help-encode-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-encode-verbose-light.avif" alt="av1an encode --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an encode --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-concatenate-light.avif"><picture><source srcset="media/tui/help-concatenate-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-concatenate-light.avif" alt="av1an concatenate --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an concatenate --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-concatenate-verbose-light.avif"><picture><source srcset="media/tui/help-concatenate-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-concatenate-verbose-light.avif" alt="av1an concatenate --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an concatenate --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-quality-check-light.avif"><picture><source srcset="media/tui/help-quality-check-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-quality-check-light.avif" alt="av1an quality-check --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an quality-check --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-quality-check-verbose-light.avif"><picture><source srcset="media/tui/help-quality-check-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-quality-check-verbose-light.avif" alt="av1an quality-check --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an quality-check --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-clean-light.avif"><picture><source srcset="media/tui/help-clean-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-clean-light.avif" alt="av1an clean --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an clean --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-clean-verbose-light.avif"><picture><source srcset="media/tui/help-clean-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-clean-verbose-light.avif" alt="av1an clean --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an clean --help --verbose</p>
    </td>
  </tr>
</table>

## Edit Property Help

The property list of each command's `set` subcommand — the configuration-editing discovery surface.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-detect-scenes-set-light.avif"><picture><source srcset="media/tui/help-detect-scenes-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-detect-scenes-set-light.avif" alt="av1an detect-scenes set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-scenes set --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-scale-noise-set-light.avif"><picture><source srcset="media/tui/help-scale-noise-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-scale-noise-set-light.avif" alt="av1an scale-noise set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an scale-noise set --help</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-benchmark-set-light.avif"><picture><source srcset="media/tui/help-benchmark-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-benchmark-set-light.avif" alt="av1an benchmark set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an benchmark set --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-optimize-bitrate-set-light.avif"><picture><source srcset="media/tui/help-optimize-bitrate-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-optimize-bitrate-set-light.avif" alt="av1an optimize-bitrate set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an optimize-bitrate set --help</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-detect-noise-set-light.avif"><picture><source srcset="media/tui/help-detect-noise-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-detect-noise-set-light.avif" alt="av1an detect-noise set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-noise set --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-target-quality-set-light.avif"><picture><source srcset="media/tui/help-target-quality-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-target-quality-set-light.avif" alt="av1an target-quality set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an target-quality set --help</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-scale-speed-set-light.avif"><picture><source srcset="media/tui/help-scale-speed-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-scale-speed-set-light.avif" alt="av1an scale-speed set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an scale-speed set --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-concatenate-set-light.avif"><picture><source srcset="media/tui/help-concatenate-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-concatenate-set-light.avif" alt="av1an concatenate set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an concatenate set --help</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-quality-check-set-light.avif"><picture><source srcset="media/tui/help-quality-check-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-quality-check-set-light.avif" alt="av1an quality-check set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an quality-check set --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/help-init-set-light.avif"><picture><source srcset="media/tui/help-init-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-init-set-light.avif" alt="av1an init set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an init set --help</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/help-encode-set-light.avif"><picture><source srcset="media/tui/help-encode-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/help-encode-set-light.avif" alt="av1an encode set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an encode set --help</p>
    </td>
    <td width="50%" valign="top"></td>
  </tr>
</table>

## Version

The environment report printed by `av1an --version`: which VapourSynth
plugins, encoders and quality metric libraries are installed. The verbose form
adds versions and compute-device details.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="media/tui/version-light.avif"><picture><source srcset="media/tui/version-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/version-light.avif" alt="av1an --version" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an --version</p>
    </td>
    <td width="50%" valign="top">
      <a href="media/tui/version-verbose-light.avif"><picture><source srcset="media/tui/version-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="media/tui/version-verbose-light.avif" alt="av1an --version --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an --version --verbose</p>
    </td>
  </tr>
</table>
