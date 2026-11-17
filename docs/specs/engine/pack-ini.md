# Skyrim settings in converted packs

Conversion carries `Skyrim.ini` and `SkyrimPrefs.ini` into the pack's `ini/`
directory. Sources stay read only. `SkyrimCustom.ini` is not carried yet.

## Discovery and source selection

The converter library reads no ambient Documents folder by default.
`PipelineConfig.player_ini_dir` supplies a settings folder explicitly, and
`PipelineConfig::discover_player_inis` requests automatic discovery. The CLI and
launcher request discovery. The CLI also accepts `--player-ini-dir DIR` on every
platform and `--no-player-ini` to omit capture, including MO2 sources and install
templates. Existing edited pack copies stay locally owned even when external
capture is disabled. Those two flags are mutually exclusive. Metadata rebuilds
retain existing pack settings rather than capturing external settings.

Windows discovery uses the Documents known-folder API, which resolves redirection
including OneDrive. The selected SE/AE install determines the folder beneath
`Documents/My Games`:

| Store | Folder | Positive install signal |
| --- | --- | --- |
| Steam | `Skyrim Special Edition` | `steam_api64.dll` |
| GOG | `Skyrim Special Edition GOG` | `Galaxy64.dll` |
| Epic | `Skyrim Special Edition EPIC` | `EOSSDK-Win64-Shipping.dll` |
| Microsoft Store | `Skyrim Special Edition MS` | `appxmanifest.xml` |

The [MO2 SE plugin](https://github.com/ModOrganizer2/modorganizer-game_skyrimse/blob/af9b50be81a9f6e91e817b5bc7792e2ae9aec55c/src/gameskyrimse.cpp#L37-L46)
and its [directory selection](https://github.com/ModOrganizer2/modorganizer-game_skyrimse/blob/af9b50be81a9f6e91e817b5bc7792e2ae9aec55c/src/gameskyrimse.cpp#L128-L135)
confirm the GOG/Epic signals and folder names.
[Wrye Bash's SE variants](https://github.com/wrye-bash/wrye-bash/blob/09ab76083e1583ebbf5e28be516d3e98c9327da3/Mopy/bash/game/skyrimse/__init__.py#L412-L444)
confirm all four folders, including Microsoft Store. Its
[Store detector](https://github.com/wrye-bash/wrye-bash/blob/09ab76083e1583ebbf5e28be516d3e98c9327da3/Mopy/bash/game/store_mixins.py#L80-L91)
uses the app manifest. Steam's DLL is used as a positive local signal; portable
installs can use the explicit folder option.

Discovery requires `Data/Skyrim.esm`, `SkyrimSE.exe`, and exactly one store signal.
Unknown or conflicting signals produce an advisory notice and no automatic copy
or template generation. Install folders containing `SkyrimVR.exe` or `TESV.exe`
and MO2 classic/VR instances are excluded from SE settings discovery. Existing
folders belonging to another store never select the edition. Automatic
Linux/Proton discovery remains outside this feature.

For an MO2 selection, `[General] LocalSettings` in the profile's `settings.ini`
overrides instance `[Settings] profile_local_inis`, which defaults to true.
See [MO2's profile implementation](https://github.com/ModOrganizer2/modorganizer/blob/4ba22f0d8b6f9e08e44008b8395cd774551b4ebe/src/profile.cpp#L822-L848)
and [instance defaults](https://github.com/ModOrganizer2/modorganizer/blob/4ba22f0d8b6f9e08e44008b8395cd774551b4ebe/src/settings.cpp#L202-L205).
Enabled profile settings take precedence per file. A missing profile file falls
back to the selected edition's Documents file, matching
[MO2's SE initialization](https://github.com/ModOrganizer2/modorganizer-game_skyrimse/blob/af9b50be81a9f6e91e817b5bc7792e2ae9aec55c/src/gameskyrimse.cpp#L191-L207).
A failed profile lookup skips external capture and templates; a readable edited
pack copy is retained first. An unreadable existing source file skips that file. Failed lookup stays distinct from successful absence and never permits
template generation. The converter writes only into staging, including during
MO2 fallback; MO2's own profile-initialization writes are not performed.

Each successfully absent file can use its corresponding install template when
`Data/Skyrim.esm` and `SkyrimSE.exe` confirm the selected install:

| Pack file | Install template |
| --- | --- |
| `ini/Skyrim.ini` | `Skyrim_Default.ini` |
| `ini/SkyrimPrefs.ini` | `High.ini` |

The template must exist and be readable. The fixed High preset follows the
existing RF-LOD-002 policy; retail hardware-dependent first-run choices can
differ. [DynDOLOD's documentation](https://dyndolod.info/Messages/INI-Files)
describes retail launcher initialization. Optional input/copy failures are report
notices, preserving `complete: true` and exit 0 when asset conversion succeeds.

## Decoding, privacy and reconversion

Both converter and engine accept UTF-8 with or without a BOM, UTF-16 LE/BE with
a BOM, and otherwise a lossy single-byte mapping that preserves ASCII keys.
Lossy decoding produces a diagnostic. Reads are bounded to 4 MiB. Unreadable,
oversized or structurally invalid optional inputs are skipped independently.

`conversion-manifest.json.ini_files` records a settings-folder-relative source
filename, SHA-256 of the published bytes, and origin (`player_file`,
`generated_from_install`, or `pack_copy`). Locally owned copies use `ini/<name>`.
Home paths are omitted from this metadata, including when retaining older
manifests. The `[Bethesda.net]` account section is removed during capture and
retention; other files preserve exact bytes. Filtering emits UTF-8 and hashes
the published bytes; expanded filtered output above 4 MiB is omitted with an
advisory notice. Account section names are trimmed inside the brackets just as
the engine parser trims them. Review other private custom settings before
sharing a pack.
The engine bug-report bundle includes neither the raw INIs nor the conversion
manifest; it allowlists manifest summary fields instead.

Normal reconversion refreshes unedited copies and reports changed-byte
replacements. Identical copies stay quiet. Missing sources and disabled external
capture produce notices when removing an unedited copy. A missing selected
settings folder produces a notice before the genuine-absence template fallback.
An edited pack copy is kept and reported, and remains locally owned on later
reconversions.
A metadata rebuild keeps every usable pack copy and reports each retained file.
Invalid retained files produce nonfatal notices. Missing external sources remove
unedited copies during normal reconversion. This optional metadata changes no
mesh, texture, database or cached asset format.

## Engine layers and diagnostics

The engine layers pack `Skyrim.ini`, then `SkyrimPrefs.ini`. This is MudCrab's
reader policy; it is not a claim about retail routing of every non-preference key.
Case-insensitive parsing and malformed-scalar diagnostics remain supported.

Precedence, highest first:

1. Explicit scalar CLI options.
2. Explicit `--ini` files in argument order, with later files taking precedence.
3. The player's saved in-game settings.
4. Pack INIs.
5. Built-in defaults (and pack GMST sun settings where applicable).

Any explicit `--ini` list replaces the pack file list. Omitted keys use saved
menu settings where supported, otherwise defaults, including when an explicit
file is unreadable. Pack values seed the interactive settings menu. Pack-imported
grid and LOD values are clamped to the menu's own bounds before startup uses them, with a
source diagnostic when clamped. Stream radius is at most 16, unload radius at
most 32, LOD distances at most 1,000,000 and the split multiplier at most 4.
Explicit INI and CLI values retain their existing launch-owned limits.

Saving persists only the grid/LOD fields changed in the menu; untouched fields
keep following the current pack INIs across launches. Stream and unload radii
persist together as one logical grid setting because their validation is coupled.
The four LOD values persist independently. Existing settings files retain the
choices for fields already present; omitted fields follow the current pack.
A settings file written before this change contains all five groups and therefore
keeps all five saved choices. Choose **Restore defaults**, then **Save**, to
release those saved choices and follow the pack INIs again. Restore defaults
reseeds grid/LOD controls from the current pack/default or explicit launch values,
matching the automatic Metal drawing default re-arming. Later menu edits can
still save an individual grid/LOD choice.
A saved menu change survives the next interactive launch. Explicit INI and CLI
keys remain launch owned. Automated runs retain the existing behavior of using launch
configuration without loading interactive preferences.

Per-key source diagnostics run after saved settings are applied, naming player
file, generated-from-install, pack copy, saved in-game settings, `--ini`, or CLI.
Default-only values remain in the source map and are silent at startup. Original
file labels require a matching hash of the raw published bytes. Sun values with
no override resolve later from pack GMSTs or the retail fallback.
`--sun-position` identifies the explicit fixed direction separately.
`--always-active` sets true only; an explicit INI can set `bAlwaysActive=0`.
