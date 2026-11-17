# l4d2r integration

Local integration components use the `l4d2r` namespace. The original snapshot is kept in a private local folder. That directory remains unchanged. This rewrite retains Bevy 0.19.1, the existing Windows launcher, installed-asset mounting and its main menu. The original snapshot's custom wgpu/winit frontend is not required to run the merged game.

## Adopted components

| Component | Integration |
| --- | --- |
| HUD | l4d2r's 480-unit VGUI layout, native Stubble/Toolbox fonts, textured health panels, portraits and sliced weapon/equipment frames; our live health, incapacitation, ammunition and owned inventory remain authoritative. |
| Animation | Vendored studio skeleton/sequence/include/ANI decoding and fractional sampling; independent Bevy meshes per actor; native idle/locomotion, deploy/fire/reload and merged Ellis arm palettes. Sniper activities are resolved with their native suffixes. |
| Item display | Equipped first-aid kit and pain pills use their native viewmodels; existing primary/secondary models are retained, including the hunting rifle's native idle sequence. Generic gun spawns replace the BSP ammo-stack placeholder with a gun from their `weapon_selection` group, shared by rendering and pickup logic. Selection within a group is deterministic and provisional. |
| Navigation | l4d2r A* over the installed NAV file drives survivor route waypoints, combined with our hull movement, support checks, following and combat. Survivors can request nearby unlocked doors to open. |
| Collision | l4d2r BSP tree tracing, moveable/window/grate/player-clip masks, displacement prisms and parsed PHY convexes for supported props; entity colliders follow map I/O. Model-bound boxes remain a fallback. |
| Map I/O / intro | l4d2r entity outputs, delayed inputs, doors and breakables; hotel `OnGameplayStart`, scripted helicopter poses, animated camera attachments and screen fades; camera/input/HUD return after the intro. E operates nearby usable doors/buttons; gunfire damages registered breakables. Ambient sound events use our lazy native sound bank. |
| Door meshes / eyes | Native closed/idle door bone poses, plus neutral iris projection from the MDL eyeball records. |

Vendored formats and entity/tracing modules live under `vendor/`; native asset files and binary research outputs are not bundled. Source file hashes are recorded in `vendor/L4D2R_PROVENANCE.json`. No LICENSE file was found in the supplied snapshot; that metadata does not infer a publishing license.

## Validation


## Fidelity boundaries

This is an integration of two incomplete runtimes, not a completed 1:1 game. l4d2r explicitly labels parts of its behavior as stand-ins. Its sequence autolayers, facial flex/eye targeting, IK and procedural bones are still incomplete. Neutral iris projection is an approximation; [Valve's public studio header](https://raw.githubusercontent.com/ValveSoftware/source-sdk-2013/master/src/public/studio.h) supports the eyeball record layout, not parity with the proprietary EyeRefract shader. Rotating brush/door collision uses bounding boxes in some cases; rigid-body impulses, fragments and all damage rules are not VPhysics parity. Generic `weapon_item_spawn` item flags/Director weights, NAV path costs/ladder traversal, Director/VScript, finales, multiplayer, choreographed dialogue and full campaign transitions remain unfinished. Existing squad/infected combat still supplies the gameplay state.
