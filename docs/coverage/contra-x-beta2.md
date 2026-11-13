## Coverage: Contra X Beta 2

| Check | Result |
|---|---|
| Load order | 158 startup files, 3 on demand, 294 map override files; 0 required files missing |
| INI parsed | 453 files, 1177066 lines, 31515 definitions |
| Structure (blocks and `End`) | 0 problems |
| Schema (fields and modules known) | 847388 fields, 0 unknown |
| Values | 739157 parsed (99.9%), 0 invalid, 978 use 36 parsers not yet ported |
| References | 102167 total, 196 names that are never defined |
| Module classes used | 203 (simulation not implemented yet: M4) |
| Files never loaded | 8 |

### References by kind

| Kind | Uses | Distinct names | Undefined |
|---|---|---|---|
| Object | 1221 | 983 | 0 |
| Armor | 3475 | 197 | 0 |
| Weapon | 5550 | 1915 | 2 |
| FxList | 8352 | 805 | 5 |
| ParticleSystem | 38312 | 906 | 3 |
| DamageFx | 2104 | 20 | 0 |
| ObjectCreationList | 9721 | 1424 | 6 |
| Upgrade | 244 | 118 | 0 |
| SpecialPower | 2814 | 227 | 1 |
| Science | 2951 | 244 | 1 |
| Audio | 13966 | 1960 | 10 |
| MappedImage | 919 | 166 | 12 |
| Label | 3927 | 1209 | 102 |
| CommandButton | 6081 | 1642 | 53 |
| Locomotor | 2109 | 397 | 0 |
| Crate | 421 | 7 | 1 |

Undefined names (first use shown; the original treats these as null, or shows `MISSING:` for labels):

- Weapon `CINE_GLARebelMachineGun` at Maps\gla02\map.ini:897
- Weapon `RadarVanBogusWeapon` at Maps\md_chi01\map.ini:85
- FxList `FX_CINE_CarpetBomb` at Maps\md_gla03\map.ini:221
- FxList `FX_ToxinTractorDie` at Data\INI\Object\cine_objects.ini:14083
- FxList `FX_WaterPropExplodeLow` at Data\INI\Object\civilianbuilding.ini:16970
- FxList `INITIAL` at Data\INI\Object\chem_glasystem.ini:1663
- FxList `WeaponFX_MediumPoisonFieldWeapon` at Maps\gla06\map.ini:88
- ParticleSystem `DamMist_Big_Black` at Maps\desert vortex\map.ini:142
- ParticleSystem `DamMist_Grey` at Maps\desert vortex\map.ini:136
- ParticleSystem `DrainPipeWater2` at Maps\sakura forest ii\map.ini:51
- ObjectCreationList `FINAL` at Data\INI\Object\bossgeneral.ini:22178
- ObjectCreationList `INITIAL` at Data\INI\Object\chem_glasystem.ini:1664
- ObjectCreationList `OCL_CINE_U05_TechnicalAirDeathStart` at Data\INI\Object\cine_objects.ini:3947
- ObjectCreationList `OCL_CINE_U05_TechnicalAirDeathStart02` at Data\INI\Object\cine_objects.ini:4354
- ObjectCreationList `OCL_FinalHumveeDebris` at Data\INI\Object\cine_objects.ini:5457
- ObjectCreationList `OCL_InitialHumveeDebris` at Data\INI\Object\cine_objects.ini:5455
- SpecialPower `SuperweaponChinaCarpetBomb` at Maps\md_chi02\map.ini:101
- Science `SCIENCE_DaisyCutter` at Maps\gla05\map.ini:88
- Audio `Amb_VolcanoEruption` at Data\INI\Object\civilianunit.ini:5235
- Audio `CINE_SatSound01Loop` at Data\INI\Object\cine_objects.ini:1455
- Audio `CombatCycleSwitchRider` at Data\INI\Object\assaultgeneral.ini:6970
- Audio `EmergencyHealingActivate` at Data\INI\SpecialPower.ini:2332
- Audio `LaserStrikeSound` at Data\INI\Object\lasergeneral.ini:20642
- Audio `RebelVoiceUpgradeCamoflage` at Maps\md_gla04_intro\map.ini:35
- Audio `StingerSoldierVoiceAttack` at Data\INI\Object\cine_objects.ini:8894
- Audio `StingerSoldierVoiceMove` at Data\INI\Object\cine_objects.ini:8893
- Audio `StingerSoldierVoiceSelect` at Data\INI\Object\cine_objects.ini:8892
- Audio `Waterfall1AmbientLoop` at Data\INI\Object\civilianprop.ini:9343
- MappedImage `NONE` at Data\INI\ControlBarScheme.ini:336
- MappedImage `SABarButtonGen2IN` at Data\INI\Default\ControlBarScheme.ini:52
- MappedImage `SABarButtonGen2ON` at Data\INI\Default\ControlBarScheme.ini:53
- MappedImage `SCBigButton` at Data\INI\Default\ControlBarScheme.ini:43
- MappedImage `SNBarButtonGen2IN` at Data\INI\ControlBarScheme.ini:249
- MappedImage `SNBarButtonGen2ON` at Data\INI\ControlBarScheme.ini:250
- MappedImage `SUBarButtonGen2IN` at Data\INI\ControlBarScheme.ini:153
- MappedImage `SUBarButtonGen2ON` at Data\INI\ControlBarScheme.ini:154
- MappedImage `SUMinMax` at Data\INI\ControlBarScheme.ini:193
- MappedImage `SUMinMaxH` at Data\INI\ControlBarScheme.ini:194
- MappedImage `SUMinMaxP` at Data\INI\ControlBarScheme.ini:195
- MappedImage `UnitBackgroundA` at Data\INI\Default\ControlBarScheme.ini:44
- Label `CONTROLBAR:ToolTipChinaScienceNukeCannon` at Maps\chi01\map.ini:148
- Label `CONTROLBAR:ToolTipGLAScienceHijacker` at Maps\gla01\map.ini:212
- Label `CONTROLBAR:ToolTipGLAScienceSCUDLauncher` at Maps\gla01\map.ini:187
- Label `CONTROLBAR:ToolTipGLaScienceMarauderTank` at Maps\gla01\map.ini:195
- Label `CONTROLBAR:ToolTipUSASciencePaladin` at Maps\md_usa03_end1\map.ini:221
- Label `CONTROLBAR:ToolTipUSASciencePathFinder` at Maps\md_usa03_end1\map.ini:246
- Label `CONTROLBAR:ToolTipUSAScienceStealthFighter` at Maps\md_usa03_end1\map.ini:229
- Label `GUI:AddTeam0` at Data\english\CommandMap.ini:379
- Label `GUI:AddTeam0Description` at Data\english\CommandMap.ini:378
- Label `GUI:AddTeam1` at Data\english\CommandMap.ini:389
- Label `GUI:AddTeam1Description` at Data\english\CommandMap.ini:388
- Label `GUI:AddTeam2` at Data\english\CommandMap.ini:399
- Label `GUI:AddTeam2Description` at Data\english\CommandMap.ini:398
- Label `GUI:AddTeam3` at Data\english\CommandMap.ini:409
- Label `GUI:AddTeam3Description` at Data\english\CommandMap.ini:408
- Label: 87 more
- CommandButton `Command_ConstructChinaInfantryRedguard` at Maps\md_gla03\map.ini:193
- CommandButton `Command_ConstructChinaInfantryTankHunter` at Maps\md_gla03\map.ini:194
- CommandButton `Command_ConstructChinaJetMIG` at Maps\chi05\map.ini:3
- CommandButton `Command_ConstructChinaTankBattleMaster` at Maps\chi01\map.ini:28
- CommandButton `Command_ConstructChinaTankDragon` at Maps\chi01\map.ini:30
- CommandButton `Command_ConstructChinaTankGattling` at Maps\md_gla03\map.ini:184
- CommandButton `Command_ConstructChinaVehicleTroopCrawler` at Maps\chi01\map.ini:29
- CommandButton `Command_ConstructGLAInfantryRPGTrooper` at Maps\gla02\map.ini:698
- CommandButton `Command_ConstructGLAInfantryRebel` at Maps\gla02\map.ini:697
- CommandButton `Command_ConstructGLATankMarauder` at Maps\md_chi02\map.ini:131
- CommandButton `Command_ConstructGLATankScorpion` at Maps\md_chi01\map.ini:108
- CommandButton `Command_ConstructGLAVehicleBattleBus` at Maps\md_chi03\map.ini:112
- CommandButton `Command_ConstructGLAVehicleBombTruck` at Maps\md_chi02\map.ini:132
- CommandButton `Command_ConstructGLAVehicleCombatBike` at Maps\md_chi01\map.ini:115
- CommandButton `Command_ConstructGLAVehicleCombatBikeRocket` at Maps\md_chi01\map.ini:118
- CommandButton: 38 more
- Crate `USA02_Always2500DollarCrate` at Maps\md_usa02\map.ini:97

### Value parsers not yet ported

`Anim2DTemplate::parseImage` (170), `parseFrictionPerSec` (111), `parseAmount` (82), `parseRiderInfo` (81), `parseFactionObjectCreationList` (75), `NameKeyGenerator::parseStringAsNameKeyType` (74), `parseAllVetLevelsPSys` (42), `parseAnimSet` (40), `parseCashHackUpgradePair` (38), `Eva::parseEvaMessageFromIni` (30), `parseInitialPayload` (26), `parseTime` (22), `GlobalLanguage::parseFontDesc` (17), `PlayerTemplate::parseStartMoney` (17), `TurretAIData::parseTurretSweep` (16), `Anim2DTemplate::parseNumImages` (14), `TurretAIData::parseTurretSweepSpeed` (14), `parseAppendQuantityModifier` (13), `parseTransitionToFX` (12), `parseTransitionToOCL` (12), `parseAngleFX` (11), `BoneFXUpdateModuleData::parseParticleSystem` (8), `parseRunwayStrip` (8), `parseArbitraryFXIntoMap` (7), `CrateTemplate::parseCrateCreationEntry` (6), `parsePerVetLevelFXList` (6), `parseUpgradePair` (6), `Money::parseMoneyAmount` (5), `parsePerVetLevelPSys` (4), `parseHeightToSpeed` (3), `BoneFXUpdateModuleData::parseFXList` (2), `parseSpeakerType` (2), `GlobalLanguage::parseFontFileName` (1), `parseInitialRoster` (1), `parseInt` (1), `parsePercentToReal` (1)

### Files never loaded

- Data\INI\commandmapdebug.ini
- Data\INI\commandmapdemo.ini
- Data\INI\controlbarresizer.ini
- Data\INI\default\buttonsets.ini
- Data\INI\default\webpages.ini
- Data\INI\defensetowers.ini
- Data\INI\gamedatadebug.ini
- Data\INI\memorypools.ini

### Module classes by use

W3DModelDraw (5323), InstantDeathBehavior (2753), ActiveBody (2605), SlowDeathBehavior (2243), PhysicsBehavior (1993), OCLSpecialPower (1942), DestroyDie (1491), FireWeaponWhenDeadBehavior (1347), TransitionDamageFX (1300), FlammableUpdate (1227), AIUpdateInterface (1187), FXListDie (1084), CreateObjectDie (855), LifetimeUpdate (841), StructureBody (737), FireWeaponWhenDamagedBehavior (652), FireWeaponUpdate (605), ObjectCreationUpgrade (536), ProductionUpdate (534), CreateCrateDie (421), StealthUpdate (364), AnimatedParticleSysBoneClientUpdate (308), DeletionUpdate (290), VeterancyGainCreate (286), MissileAIUpdate (279), CommandSetUpgrade (263), W3DTruckDraw (254), ArmorUpgrade (247), SquishCollide (236), GarrisonContain (230), AutoHealBehavior (221), ModelConditionUpgrade (214), OCLUpdate (189), StealthDetectorUpdate (187), WeaponSetUpgrade (181), GrantUpgradeCreate (179), HeightDieUpdate (155), ImmortalBody (153), MaxHealthUpgrade (152), W3DDefaultDraw (152), FireWeaponCollide (140), StealthUpgrade (137), LocomotorSetUpgrade (135), CommandButtonHuntUpdate (129), WeaponBonusUpgrade (128), DumbProjectileBehavior (124), SpecialAbility (119), SpecialAbilityUpdate (117), BaseRegenerateUpdate (114), TransportContain (110), CashBountyPower (107), W3DOverlordTankDraw (107), W3DTankDraw (107), OverlordContain (105), W3DLaserDraw (101), RebuildHoleExposeDie (97), HordeUpdate (92), DefaultProductionExitUpdate (90), GenerateMinefieldBehavior (90), JetSlowDeathBehavior (90), ChinookAIUpdate (89), ExperienceScalarUpgrade (82), W3DOverlordAircraftDraw (80), AutoFindHealingUpdate (76), LockWeaponCreate (76), KeepObjectDie (75), LaserUpdate (74), CountermeasuresBehavior (69), W3DDependencyModelDraw (69), JetAIUpdate (62), ToppleUpdate (60), HighlanderBody (59), SpecialPowerCreate (59), SubObjectsUpgrade (59), StatusBitsUpgrade (57), TransportAIUpdate (56), UpgradeDie (55), RailroadBehavior (52), SpawnBehavior (50), W3DScienceModelDraw (48), TechBuildingBehavior (46), UnpauseSpecialPowerUpgrade (44), SlavedUpdate (43), CrushDie (41), W3DTankTruckDraw (41), DeliverPayloadAIUpdate (40), OverchargeBehavior (39), InactiveBody (36), MissileLauncherBuildingUpdate (36), DeployStyleAIUpdate (31), W3DOverlordTruckDraw (31), CleanupHazardUpdate (30), MoneyCrateCollide (30), CleanupAreaPower (29), PropagandaTowerBehavior (29), HelicopterSlowDeathBehavior (28), WanderAIUpdate (28), FireSpreadUpdate (27), GrantScienceUpgrade (27), RiderChangeContain (27), AnimationSteeringUpdate (25), RadiusDecalUpdate (25), RepairDockUpdate (24), WorkerAIUpdate (24), HelixContain (23), EMPUpdate (22), BoneFXDamage (21), BoneFXUpdate (21), PoisonedBehavior (21), QueueProductionExitUpdate (21), SpecialPowerCompletionDie (21), SupplyCenterCreate (21), SupplyCenterProductionExitUpdate (21), StructureToppleUpdate (20), SwayClientUpdate (20), CashHackSpecialPower (19), HiveStructureBody (19), SupplyCenterDockUpdate (19), BaikonurLaunchPower (18), HealContain (18), StickyBombUpdate (18), FireOCLAfterWeaponCooldownUpdate (17), MobMemberSlavedUpdate (17), RadarUpgrade (16), StructureCollapseUpdate (16), AutoDepositUpdate (15), ParkingPlaceBehavior (15), PowerPlantUpgrade (15), WeaponBonusUpdate (15), DemoTrapUpdate (14), DozerAIUpdate (14), NeutronMissileSlowDeathBehavior (14), NeutronMissileUpdate (13), SalvageCrateCollide (13), PowerPlantUpdate (11), EjectPilotDie (10), RadarUpdate (10), TunnelContain (10), MinefieldBehavior (9), ProjectileStreamUpdate (9), W3DProjectileStreamDraw (9), SupplyTruckAIUpdate (8), BridgeBehavior (7), ConvertToHijackedVehicleCrateCollide (7), HackInternetAIUpdate (7), SabotageSupplyCenterCrateCollide (7), SabotageSupplyDropzoneCrateCollide (7), SpyVisionUpdate (7), W3DSupplyDraw (7), AssistedTargetingUpdate (6), BattlePlanUpdate (6), ParachuteContain (6), PointDefenseLaserUpdate (6), SpyVisionSpecialPower (6), W3DTreeDraw (6), DynamicShroudClearingRangeUpdate (5), FloatUpdate (5), InternetHackContain (5), ParticleUplinkCannonUpdate (5), SupplyWarehouseCreate (5), SupplyWarehouseDockUpdate (5), VeterancyCrateCollide (5), BridgeScaffoldBehavior (4), FirestormDynamicGeometryInfoUpdate (4), HijackerUpdate (4), ReplaceObjectUpgrade (4), SabotageCommandCenterCrateCollide (4), SabotageMilitaryFactoryCrateCollide (4), SabotagePowerPlantCrateCollide (4), SabotageSuperweaponCrateCollide (4), SpawnPointProductionExitUpdate (4), UnitCrateCollide (4), ActiveShroudUpgrade (3), BunkerBusterBehavior (3), CostModifierUpgrade (3), GrantStealthBehavior (3), PilotFindVehicleUpdate (3), W3DPoliceCarDraw (3), ConvertToCarBombCrateCollide (2), FlightDeckBehavior (2), NeutronBlastBehavior (2), RebuildHoleBehavior (2), TensileFormationUpdate (2), WaveGuideUpdate (2), AssaultTransportAIUpdate (1), BattleBusSlowDeathBehavior (1), BeaconClientUpdate (1), BridgeTowerBehavior (1), CheckpointUpdate (1), DamDie (1), EnemyNearUpdate (1), FireWeaponPower (1), PassengersFireUpgrade (1), PreorderCreate (1), RailedTransportAIUpdate (1), RailedTransportContain (1), RailedTransportDockUpdate (1), SmartBombTargetHomingUpdate (1), SupplyWarehouseCripplingBehavior (1), UndeadBody (1), W3DDebrisDraw (1), W3DRopeDraw (1), W3DTracerDraw (1)
