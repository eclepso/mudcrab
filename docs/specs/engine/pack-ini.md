# Skyrim settings in converted packs

Conversion carries `Skyrim.ini` and `SkyrimPrefs.ini` into the pack's `ini/`
directory. On Windows, discovery uses the Documents known folder and appends
`My Games/Skyrim Special Edition`; Windows resolves redirected Documents,
including OneDrive. Source files are read only. Other platforms can supply the
`PipelineConfig.player_ini_dir` settings directory; automatic Proton discovery
remains outside this feature.

When an MO2 profile is selected, its profile-specific INIs take precedence over
Documents when `[General] LocalSettings` in the profile's `settings.ini` is
enabled. An absent override follows the instance's `[Settings] profile_local_inis`
setting, which defaults to true, matching [MO2's profile implementation](https://github.com/ModOrganizer2/modorganizer/blob/master/src/profile.cpp)
and [instance settings](https://github.com/ModOrganizer2/modorganizer/blob/master/src/settings.cpp).
An unreadable profile setting produces a warning and skips player-file discovery.

Each missing file can use the selected install's template only when the Data
folder contains `Skyrim.esm` and its parent contains `SkyrimSE.exe`:

| Pack file | Install template |
| --- | --- |
| `ini/Skyrim.ini` | `Skyrim_Default.ini` |
| `ini/SkyrimPrefs.ini` | `High.ini` |

The corresponding template must exist and be readable. An existing player file
is preserved independently of the other file. Unreadable or malformed inputs
warn and are skipped; they leave usable conversion outputs intact. With neither
player files nor a confirmed install, conversion writes no INIs.

[DynDOLOD's INI documentation](https://dyndolod.info/Messages/INI-Files) describes
running the retail launcher once to create the default INIs. The install's
High template supplies an equivalent fixed High preset here, consistent with
RF-LOD-002's built-in High terrain settings. The retail launcher also applies
baseline and hardware-dependent settings, so this copy is not byte-identical to
every first-run launcher output. [Step's game-file reference](https://stepmodifications.org/wiki/SkyrimSE:Game_Files)
describes the preferences baseline and the preset files.

`conversion-manifest.json` records `ini_files`, keyed by pack-relative path, with
the original `source`, exact copied-byte `sha256`, and `origin` (`player_file` or
`generated_from_install`, or `pack_copy` for retained local edits). This optional metadata extends the existing manifest;
it changes no mesh, texture, database, or cache format. Normal conversion refreshes
the copies even when converted assets are reused, removing stale INIs whose
sources disappeared. Metadata-only rebuilding retains the existing pack settings.
Windows provenance and INI diagnostics use readable drive/UNC paths with consistent
separators; paths whose special namespace affects their meaning retain that spelling.

The engine loads `ini/Skyrim.ini` followed by `ini/SkyrimPrefs.ini` automatically
through its existing Skyrim INI reader. Supported keys retain the reader's
case-insensitive parsing and malformed-scalar warnings.

Precedence, from highest to lowest:

1. Explicit scalar CLI options, wherever they appear in the argument list.
2. Explicit `--ini` files, in argument order; later files override earlier files.
3. Pack INIs, in the order above.
4. Engine defaults (and pack GMST sun settings where applicable).

Providing any `--ini` replaces the entire pack INI list. Its omitted keys fall
back to defaults, even if the explicit file is missing or unreadable. This makes
an explicit list reproducible independently of the pack's saved player settings.

Startup diagnostics report supported launch values and their origins: player file,
generated-from-install, pack copy, `--ini`, CLI, or default. Original file labels
are used when the pack file's bytes still match its recorded hash; locally edited
copies and packs lacking INI metadata use the pack-copy label. Sun settings with
no INI override resolve later from the pack's GMSTs or the retail fallback;
`--sun-position` reports the explicitly fixed direction separately.
Saved interactive rendering preferences apply afterward to values with no INI or
CLI owner; imported INI values and explicit CLI options retain precedence.
