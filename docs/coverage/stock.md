## Coverage: Stock Zero Hour

| Check | Result |
|---|---|
| Load order | 132 startup files, 3 on demand, 82 map override files; 0 required files missing |
| INI parsed | 215 files, 568497 lines, 13825 definitions |
| Structure (blocks and `End`) | 0 problems |
| Schema (fields and modules known) | 386620 fields, 0 unknown |
| Values | 333898 parsed (99.7%), 0 invalid, 840 use 34 parsers not yet ported |
| References | 51246 total, 60 names that are never defined |
| Module classes used | 205 (simulation not implemented yet: M4) |
| Files never loaded | 7 |

### References by kind

| Kind | Uses | Distinct names | Undefined |
|---|---|---|---|
| Object | 508 | 469 | 1 |
| Armor | 1387 | 52 | 0 |
| Weapon | 1452 | 337 | 0 |
| FxList | 4171 | 355 | 2 |
| ParticleSystem | 23888 | 284 | 0 |
| DamageFx | 1076 | 11 | 0 |
| ObjectCreationList | 2402 | 228 | 4 |
| Upgrade | 105 | 76 | 0 |
| SpecialPower | 705 | 74 | 0 |
| Science | 543 | 76 | 0 |
| Audio | 7082 | 912 | 2 |
| MappedImage | 659 | 157 | 11 |
| Label | 2408 | 600 | 39 |
| CommandButton | 3621 | 764 | 0 |
| Locomotor | 973 | 173 | 0 |
| Crate | 266 | 7 | 1 |

Undefined names (first use shown; the original treats these as null, or shows `MISSING:` for labels):

- Object `GC_Nuke_ChinaCarpetBomb` at Data\INI\ObjectCreationList.ini:5094
- FxList `DieByFireUSA` at Data\INI\Object\civilianunit.ini:9395
- FxList `FINAL` at Data\INI\Object\weaponobjects.ini:5838
- ObjectCreationList `Chem_PoisonFieldGammaLarge` at Data\INI\Object\chemicalgeneral.ini:11976
- ObjectCreationList `OCL_` at Data\INI\Object\chinamiscunit.ini:657
- ObjectCreationList `OCL_AmericanPOWTruckDeathEffect` at Data\INI\Object\americacineunit.ini:2669
- ObjectCreationList `OCL_SaboteurFlyingOffBikeDeathStart` at Data\INI\Object\chemicalgeneral.ini:15543
- Audio `BriefingUSATraining` at Data\INI\Campaign.ini:22
- Audio `RebelVoiceUpgradeCamoflage` at Maps\md_gla04_intro\map.ini:35
- MappedImage `SABarButtonGen2IN` at Data\INI\Default\ControlBarScheme.ini:52
- MappedImage `SABarButtonGen2ON` at Data\INI\Default\ControlBarScheme.ini:53
- MappedImage `SCBigButton` at Data\INI\Default\ControlBarScheme.ini:43
- MappedImage `SNBarButtonGen2IN` at Data\INI\ControlBarScheme.ini:235
- MappedImage `SNBarButtonGen2ON` at Data\INI\ControlBarScheme.ini:236
- MappedImage `SUBarButtonGen2IN` at Data\INI\ControlBarScheme.ini:146
- MappedImage `SUBarButtonGen2ON` at Data\INI\ControlBarScheme.ini:147
- MappedImage `SUMinMax` at Data\INI\ControlBarScheme.ini:186
- MappedImage `SUMinMaxH` at Data\INI\ControlBarScheme.ini:187
- MappedImage `SUMinMaxP` at Data\INI\ControlBarScheme.ini:188
- MappedImage `UnitBackgroundA` at Data\INI\Default\ControlBarScheme.ini:44
- Label `GUI:AddTeam0` at Data\english\CommandMap.ini:379
- Label `GUI:AddTeam0Description` at Data\english\CommandMap.ini:378
- Label `GUI:AddTeam1` at Data\english\CommandMap.ini:389
- Label `GUI:AddTeam1Description` at Data\english\CommandMap.ini:388
- Label `GUI:AddTeam2` at Data\english\CommandMap.ini:399
- Label `GUI:AddTeam2Description` at Data\english\CommandMap.ini:398
- Label `GUI:AddTeam3` at Data\english\CommandMap.ini:409
- Label `GUI:AddTeam3Description` at Data\english\CommandMap.ini:408
- Label `GUI:AddTeam4` at Data\english\CommandMap.ini:419
- Label `GUI:AddTeam4Description` at Data\english\CommandMap.ini:418
- Label `GUI:AddTeam5` at Data\english\CommandMap.ini:429
- Label `GUI:AddTeam5Description` at Data\english\CommandMap.ini:428
- Label `GUI:AddTeam6` at Data\english\CommandMap.ini:439
- Label `GUI:AddTeam6Description` at Data\english\CommandMap.ini:438
- Label `GUI:AddTeam7` at Data\english\CommandMap.ini:449
- Label: 24 more
- Crate `USA02_Always2500DollarCrate` at Maps\md_usa02\map.ini:97

### Value parsers not yet ported

`Anim2DTemplate::parseImage` (170), `parseAllVetLevelsPSys` (89), `parseRiderInfo` (77), `parsePerVetLevelPSys` (75), `parsePerVetLevelFXList` (61), `parseFrictionPerSec` (48), `Eva::parseEvaMessageFromIni` (38), `NameKeyGenerator::parseStringAsNameKeyType` (25), `TurretAIData::parseTurretSweep` (22), `BoneFXUpdateModuleData::parseParticleSystem` (20), `parseAnimSet` (20), `parseInitialPayload` (19), `TurretAIData::parseTurretSweepSpeed` (18), `GlobalLanguage::parseFontDesc` (17), `PlayerTemplate::parseStartMoney` (15), `Anim2DTemplate::parseNumImages` (14), `parseFactionObjectCreationList` (12), `parseTransitionToFX` (12), `parseTransitionToOCL` (12), `CrateTemplate::parseCrateCreationEntry` (11), `parseCashHackUpgradePair` (10), `parseAmount` (9), `parseTime` (9), `parseArbitraryFXIntoMap` (7), `parseAngleFX` (6), `Money::parseMoneyAmount` (5), `parseAppendQuantityModifier` (5), `parseRunwayStrip` (4), `BoneFXUpdateModuleData::parseFXList` (2), `parseInitialRoster` (2), `parseSpeakerType` (2), `parseUpgradePair` (2), `parseInt` (1), `parsePercentToReal` (1)

### Files never loaded

- Data\INI\commandmapdebug.ini
- Data\INI\commandmapdemo.ini
- Data\INI\controlbarresizer.ini
- Data\INI\default\buttonsets.ini
- Data\INI\default\webpages.ini
- Data\INI\gamedatadebug.ini
- Data\INI\memorypools.ini

### Module classes by use

W3DModelDraw (2342), SlowDeathBehavior (1494), ActiveBody (1193), FXListDie (1073), FlammableUpdate (827), DestroyDie (756), PhysicsBehavior (708), TransitionDamageFX (698), CreateObjectDie (572), AIUpdateInterface (511), StructureBody (315), CreateCrateDie (266), ProductionUpdate (245), GarrisonContain (213), FireWeaponWhenDeadBehavior (209), OCLSpecialPower (200), PoisonedBehavior (156), ArmorUpgrade (154), SquishCollide (153), W3DDefaultDraw (150), LifetimeUpdate (143), StealthUpdate (133), ObjectCreationUpgrade (132), W3DTreeDraw (128), InstantDeathBehavior (126), CommandSetUpgrade (124), WeaponSetUpgrade (119), W3DTruckDraw (118), SpecialAbility (115), StealthDetectorUpdate (104), SpecialAbilityUpdate (100), AutoHealBehavior (90), ExperienceScalarUpgrade (87), CommandButtonHuntUpdate (85), W3DTankDraw (75), TransportContain (73), KeepObjectDie (66), FireWeaponUpdate (63), GenerateMinefieldBehavior (63), WeaponBonusUpgrade (62), RebuildHoleExposeDie (61), JetSlowDeathBehavior (59), GrantUpgradeCreate (58), BaseRegenerateUpdate (55), ModelConditionUpgrade (55), MaxHealthUpgrade (52), VeterancyGainCreate (51), EjectPilotDie (48), ToppleUpdate (48), CrushDie (42), DefaultProductionExitUpdate (42), MissileAIUpdate (42), CountermeasuresBehavior (40), UnpauseSpecialPowerUpgrade (38), ImmortalBody (36), SpawnBehavior (35), AutoFindHealingUpdate (34), JetAIUpdate (34), AnimatedParticleSysBoneClientUpdate (32), HeightDieUpdate (32), DeliverPayloadAIUpdate (29), RailroadBehavior (28), DeletionUpdate (27), W3DDependencyModelDraw (26), StealthUpgrade (25), W3DLaserDraw (25), LaserUpdate (24), TransportAIUpdate (24), BaikonurLaunchPower (23), PointDefenseLaserUpdate (23), SpecialPowerCompletionDie (22), SpecialPowerCreate (22), CashBountyPower (21), ReplaceObjectUpgrade (20), SlavedUpdate (20), PreorderCreate (19), HelicopterSlowDeathBehavior (18), LocomotorSetUpgrade (18), MoneyCrateCollide (18), RepairDockUpdate (17), SubObjectsUpgrade (17), DumbProjectileBehavior (16), AutoDepositUpdate (15), HealContain (15), HighlanderBody (15), HiveStructureBody (15), MobMemberSlavedUpdate (15), RadarUpgrade (15), SupplyCenterCreate (15), SupplyCenterDockUpdate (15), SupplyCenterProductionExitUpdate (15), PropagandaTowerBehavior (14), HelixContain (13), HordeUpdate (13), UpgradeDie (13), FireOCLAfterWeaponCooldownUpdate (12), SpyVisionUpdate (12), TechBuildingBehavior (12), TunnelContain (12), W3DOverlordAircraftDraw (12), AnimationSteeringUpdate (11), ChinookAIUpdate (11), LockWeaponCreate (11), QueueProductionExitUpdate (11), RiderChangeContain (11), BoneFXDamage (10), BoneFXUpdate (10), DeployStyleAIUpdate (10), DozerAIUpdate (10), OverlordContain (10), StructureCollapseUpdate (10), ParkingPlaceBehavior (9), PowerPlantUpdate (9), RadarUpdate (9), SpectreGunshipDeploymentUpdate (9), ConvertToCarBombCrateCollide (8), FireWeaponWhenDamagedBehavior (8), ParticleUplinkCannonUpdate (8), WorkerAIUpdate (8), BridgeBehavior (7), DemoTrapUpdate (7), InactiveBody (7), MissileLauncherBuildingUpdate (7), NeutronMissileSlowDeathBehavior (7), ParachuteContain (7), ProjectileStreamUpdate (7), RadiusDecalUpdate (7), SpawnPointProductionExitUpdate (7), SpectreGunshipUpdate (7), VeterancyCrateCollide (7), W3DProjectileStreamDraw (7), W3DTankTruckDraw (7), BattleBusSlowDeathBehavior (6), CleanupAreaPower (6), CleanupHazardUpdate (6), ConvertToHijackedVehicleCrateCollide (6), DynamicShroudClearingRangeUpdate (6), HijackerUpdate (6), StructureToppleUpdate (6), SupplyTruckAIUpdate (6), UndeadBody (6), AssaultTransportAIUpdate (5), AssistedTargetingUpdate (5), CashHackSpecialPower (5), EMPUpdate (5), FloatUpdate (5), HackInternetAIUpdate (5), OCLUpdate (5), PassengersFireUpgrade (5), PilotFindVehicleUpdate (5), PowerPlantUpgrade (5), SalvageCrateCollide (5), SupplyWarehouseCreate (5), SupplyWarehouseDockUpdate (5), W3DOverlordTankDraw (5), W3DOverlordTruckDraw (5), W3DSupplyDraw (5), BattlePlanUpdate (4), BridgeScaffoldBehavior (4), FireSpreadUpdate (4), FireWeaponCollide (4), GrantScienceUpgrade (4), InternetHackContain (4), OverchargeBehavior (4), RebuildHoleBehavior (4), SpyVisionSpecialPower (4), StickyBombUpdate (4), SwayClientUpdate (4), FireWeaponPower (3), MinefieldBehavior (3), NeutronMissileUpdate (3), SabotageCommandCenterCrateCollide (3), SabotageMilitaryFactoryCrateCollide (3), SabotagePowerPlantCrateCollide (3), SabotageSuperweaponCrateCollide (3), SabotageSupplyCenterCrateCollide (3), W3DPoliceCarDraw (3), WanderAIUpdate (3), WeaponBonusUpdate (3), FirestormDynamicGeometryInfoUpdate (2), NeutronBlastBehavior (2), TensileFormationUpdate (2), WaveGuideUpdate (2), BeaconClientUpdate (1), BridgeTowerBehavior (1), BunkerBusterBehavior (1), CheckpointUpdate (1), CostModifierUpgrade (1), DamDie (1), EnemyNearUpdate (1), FlightDeckBehavior (1), GrantStealthBehavior (1), LeafletDropBehavior (1), RailedTransportAIUpdate (1), RailedTransportContain (1), RailedTransportDockUpdate (1), SabotageFakeBuildingCrateCollide (1), SabotageInternetCenterCrateCollide (1), SmartBombTargetHomingUpdate (1), SupplyWarehouseCripplingBehavior (1), UnitCrateCollide (1), W3DDebrisDraw (1), W3DRopeDraw (1), W3DScienceModelDraw (1), W3DTracerDraw (1)
