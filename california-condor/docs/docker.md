# Docker

The published legacy Av1an image is [masterofzen/av1an](https://hub.docker.com/r/masterofzen/av1an).
Do not assume that image contains this branch's Condor CLI or native metric libraries.
The workflow configuration below does not establish that new Condor images or release
tags are already available; check the registry before using a published tag.

## Publishing policy

CI is configured to publish `<DOCKERHUB_USERNAME>/av1an`, where the owner comes from
the upstream repository's `DOCKERHUB_USERNAME` secret, not a hardcoded fork owner.
Only pushes in `rust-av/Av1an` to `main` or `v*` tags can publish, after the build
and test gates pass and both Docker Hub credentials are configured:

- `main` pushes produce `:main` and `:sha-#######`, not `:latest`.
- Stable semantic version tags such as `v1.2.3` produce `:1.2.3`, `:1.2`, `:1`,
  `:latest` and a SHA tag. Prereleases follow Docker metadata-action's semver policy
  and do not update `:latest`; non-semver `v*` tags produce only a SHA tag.
- Pull requests, forks and manual runs only build with `push: false`; they never
  log in or publish. Missing credentials skip publishing, not the validation build.

## Local build and examples

To use this branch's Condor CLI, build a local image from the repository root:

```bash
docker build -t av1an:local .
```

The Dockerfile builds an Arch Linux image with the Condor CLI and native metric
libraries. These examples use that local image, not an unverified registry release.
Its entry point is `av1an` and its working directory is `/videos`, so
the arguments you pass are `av1an` arguments. The following examples assume the
file you want to encode is in your current working directory.

Linux

```bash
docker run -v "$(pwd):/videos" --user "$(id -u):$(id -g)" -it --rm av1an:local -i S01E01.mkv -o S01E01.condor.mkv
```

Windows

```powershell
docker run -v "${PWD}:/videos" -it --rm av1an:local -i S01E01.mkv -o S01E01.condor.mkv
```

Both `-i` and `-o` are given because a full run needs the input and the output
path; everything else comes from the configuration file or from the flags in
[av1an](commands/condor.md).

To specify a different directory to use you would replace $(pwd) with the directory

```bash
docker run -v "/path/to/Videos:/videos" --user "$(id -u):$(id -g)" -it --rm av1an:local -i S01E01.mkv -o S01E01.condor.mkv
```

Replace `/path/to/Videos` with your directory. On Linux, `--user` maps the process
to your user/group; ensure that user can write to the mounted directory.

These are CPU-only examples and do not require `--privileged`. They do not expose
a GPU to the container. GPU metrics need separate driver/device-specific setup;
bundling a metric library or granting blanket privileges does not establish GPU
support. Actual Docker build/run and publication acceptance remain separate CI gates.
