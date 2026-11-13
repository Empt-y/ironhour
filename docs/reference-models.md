# Reference renders

A fixed set of models to compare against the original game after renderer changes.
Renders are not committed: they are derived from the game's art. Make them with:

```
cargo run --release -p ih-tools -- sheet ABBtCmdHQ AVRaptor AVHUMMER NVOvrlrd:c81414 NVBtMstr:c81414 NICNSC_SKN:c81414 UVLiteTank:28a028 UVTechTrck:28a028 UIRGrd_SKN:28a028 --out out/sheet_m2.png
```

| Cell | Model | Object | Checks |
|---|---|---|---|
| 1 | ABBtCmdHQ | USA Command Center | Building materials, house colour trim, flag |
| 2 | AVRaptor | USA Raptor | Aircraft, additive afterburner (normally shown only when moving) |
| 3 | AVHUMMER | USA Humvee | Additive headlight beams |
| 4 | NVOvrlrd | China Overlord | Large multi-part tank |
| 5 | NVBtMstr | China Battlemaster | Tank, red house colour |
| 6 | NICNSC_SKN | China Red Guard | Skinned infantry in rest pose (the bayonet hangs at its bone) |
| 7 | UVLiteTank | GLA Scorpion | Upgrade parts visible (normally hidden until upgraded) |
| 8 | UVTechTrck | GLA Technical | Headlights, green house colour |
| 9 | UIRGrd_SKN | GLA Rebel | Skinned infantry, `ZHCA` texture house colour |

Animated and mapper checks:

```
cargo run --release -p ih-tools -- render AIRngr_SKN --hide MUZZLEFX --anim AIRngr_SKL.AIRngr_STA --frame 10 --yaw=-60 --pitch=20 --out out/ranger_idle.png
cargo run --release -p ih-tools -- render AICIAOFF_SKN --anim AICIAOFF_SKL.AICIAOFF_PLNT --frame 15 --out out/cia.png
cargo run --release -p ih-tools -- render ABARFRCCMD --out out/afcmd.png
cargo run --release -p ih-tools -- render EXEMPSPHERE --frame 15 --out out/emp.png
```

The camera defaults to a yaw of -110° and a pitch of 35°, with GameData's afternoon
object lighting. The game's own camera and the map's lighting differ, so compare
materials, colours and shapes rather than exact brightness.
