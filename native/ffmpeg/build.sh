#!/usr/bin/env bash
set -euo pipefail

root="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
out="${1:-$root/build}"
mkdir -p "$out"
manifest="$root/manifest.toml"
value() { sed -n "s/^$1[[:space:]]*=[[:space:]]*\"\([^\"]*\)\"/\1/p" "$manifest" | head -n 1; }
array_values() { sed -n "s/^$1[[:space:]]*=[[:space:]]*\[\(.*\)\]/\1/p" "$manifest" | tr -d '" ' | tr ',' '\n'; }
version="$(value version)"
url="$(value source_url)"
expected_sha="$(value source_sha256)"
patch_relative="$(value patch)"
expected_patch_sha="$(value patch_sha256)"
archive="${SOURCE_ARCHIVE:-$out/ffmpeg-$version.tar.xz}"
if [ ! -f "$archive" ]; then curl --fail --location --proto '=https' --tlsv1.2 "$url" --output "$archive"; fi
actual_sha="$(sha256sum "$archive" 2>/dev/null | awk '{print $1}' || shasum -a 256 "$archive" | awk '{print $1}')"
actual_sha_lower="$(printf '%s' "$actual_sha" | tr '[:upper:]' '[:lower:]')"
expected_sha_lower="$(printf '%s' "$expected_sha" | tr '[:upper:]' '[:lower:]')"
[ "$actual_sha_lower" = "$expected_sha_lower" ] || { echo "FFmpeg source SHA-256 mismatch" >&2; exit 1; }
command -v tar >/dev/null
command -v make >/dev/null
command -v nasm >/dev/null
source_dir="$out/src/ffmpeg-$version"
if [ ! -f "$source_dir/configure" ]; then mkdir -p "$out/src"; tar -xf "$archive" -C "$out/src"; fi
patch_file="$root/$patch_relative"
[ -f "$patch_file" ] || { echo "Required FFmpeg patch is missing: $patch_file" >&2; exit 1; }
actual_patch_sha="$(sha256sum "$patch_file" 2>/dev/null | awk '{print $1}' || shasum -a 256 "$patch_file" | awk '{print $1}')"
actual_patch_sha_lower="$(printf '%s' "$actual_patch_sha" | tr '[:upper:]' '[:lower:]')"
expected_patch_sha_lower="$(printf '%s' "$expected_patch_sha" | tr '[:upper:]' '[:lower:]')"
[ "$actual_patch_sha_lower" = "$expected_patch_sha_lower" ] || { echo "FFmpeg patch SHA-256 mismatch" >&2; exit 1; }
command -v git >/dev/null
git_ceiling="$(dirname "$source_dir")"
if GIT_CEILING_DIRECTORIES="$git_ceiling" git -C "$source_dir" apply --reverse --check "$patch_file" >/dev/null 2>&1; then
  : # The audited patch is already present in this reusable source tree.
elif GIT_CEILING_DIRECTORIES="$git_ceiling" git -C "$source_dir" apply --check "$patch_file"; then
  GIT_CEILING_DIRECTORIES="$git_ceiling" git -C "$source_dir" apply "$patch_file"
else
  echo "FFmpeg source does not match the audited Speex patch" >&2
  exit 1
fi
prefix="${FFMPEG_PREFIX:-$out/sdk}"
cd "$source_dir"
demuxers_csv="$(array_values demuxers | paste -sd, -)"
parsers_csv="$(array_values parsers | paste -sd, -)"
decoders_csv="$(array_values decoders | paste -sd, -)"
bsfs_csv="$(array_values bitstream_filters | paste -sd, -)"
configure=("--prefix=$prefix")
while IFS= read -r flag; do configure+=("$flag"); done < <(
  sed -n '/^configure[[:space:]]*=[[:space:]]*\[/,/^\]/s/^[[:space:]]*"\([^"]*\)",\{0,1\}$/\1/p' "$manifest"
)
configure+=(
  "--enable-demuxer=$demuxers_csv"
  "--enable-parser=$parsers_csv"
  "--enable-decoder=$decoders_csv"
  "--enable-bsf=$bsfs_csv"
)
if [ "$(uname -s)" = "Darwin" ]; then
  configure+=("--install-name-dir=@rpath")
fi
printf '%s\n' "${configure[@]}" | grep -Eq -- '--enable-gpl|--enable-nonfree' && { echo 'Forbidden FFmpeg feature enabled' >&2; exit 1; } || true
./configure "${configure[@]}"
python3 - config_components.h "$out/FFMPEG_COMPONENTS.json" \
  "$demuxers_csv" "$parsers_csv" "$decoders_csv" "$bsfs_csv" <<'PY'
import json, re, sys
component_path, output_path, demuxers, parsers, decoders, bsfs = sys.argv[1:]
expected = {
    "demuxers": demuxers.split(",") if demuxers else [],
    "parsers": parsers.split(",") if parsers else [],
    "decoders": decoders.split(",") if decoders else [],
    "bitstream_filters": bsfs.split(",") if bsfs else [],
}
text = open(component_path, encoding="utf-8").read()
suffixes = {"demuxers": "DEMUXER", "parsers": "PARSER", "decoders": "DECODER", "bitstream_filters": "BSF"}
actual = {}
for key, suffix in suffixes.items():
    actual[key] = sorted(name.lower() for name in re.findall(rf"^#define CONFIG_([A-Z0-9_]+)_{suffix} 1$", text, re.MULTILINE))
    if actual[key] != sorted(expected[key]):
        raise SystemExit(f"FFmpeg enabled {key} do not match manifest: expected {sorted(expected[key])}, got {actual[key]}")
with open(output_path, "w", encoding="utf-8") as output:
    json.dump(actual, output, indent=2)
PY
make -j"${JOBS:-2}"
make install
config_log="ffbuild/config.log"
[ -f "$config_log" ] || { echo 'FFmpeg did not emit ffbuild/config.log' >&2; exit 1; }
grep -Eq -- '--enable-gpl|--enable-nonfree|--enable-network|--enable-programs' "$config_log" && { echo 'Forbidden FFmpeg configuration detected' >&2; exit 1; } || true
python3 - "$out/FFMPEG_BUILD.json" "$version" "$url" "$actual_sha" "$patch_relative" "$actual_patch_sha" "$prefix" ffbuild/config.mak <<'PY'
import json, platform, sys
path, version, url, sha, patch, patch_sha, prefix, config_path = sys.argv[1:]
configuration = next(
    line.split("=", 1)[1].rstrip("\n")
    for line in open(config_path, encoding="utf-8")
    if line.startswith("FFMPEG_CONFIGURATION=")
)
json.dump({"version": version, "source_url": url, "source_sha256": sha, "patch": patch, "patch_sha256": patch_sha, "prefix": prefix, "platform": platform.platform(), "configuration": configuration}, open(path, "w", encoding="utf-8"), indent=2)
PY
cp "$patch_file" "$out/FFMPEG_CHANGES.diff"
cp "$config_log" "$out/FFMPEG_CONFIGURE.log"
echo "Built FFmpeg $version with the audited shared-only audio configuration."
