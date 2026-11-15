# Test organization and verification results

Branch: `refactor/source-aligned-tests` (local, unpushed).
Baseline: `ad238cc8b6c52153cce87e1c77e6670820847d3e`.

## Outcome

- Reviewed all **1,606 original rows**, including 23 generated cases and 38
  Python methods. Names follow asserted behavior, not historical dispatch roles.
- Reconciled **2,415 unique automated package/target/exact-name identities**.
  This is a registration count, not a claim that every platform executed.
  All current default/CUDA/screenshots/Python registrations are accounted for;
  35 mapped Windows-only identities cannot compile or execute on this Linux host.
- Moved public workflows to registered integration targets. Private-hook
  workflows retain an explicit `internal_integration` module component;
  genuine units stay with their production owner. One assertion-free exporter is
  now an explicit manual example instead of an automated test registration.
- Preserved production APIs, dependencies, native ABI and default features.
  Production Rust tokens are unchanged apart from the two approved test-helper
  assertion repairs and test-only hook/module references. The remaining installer
  edit updates its printed test-selection guidance, not installation logic.

The five approved oracle repairs comprise two actual-option assertion fixes and
their new negative checks, two CLI parser expectations, and the rescale missing-
kernel expectation. No unrelated product fixes or performance benchmarks were run.

## Verification

- `just precommit`: 2,510 reported passes, zero failures, 19 ignored gates.
  This includes default workspace and CUDA-feature tests, not real CUDA hardware
  acceptance. The aggregate includes doctests and repeated feature registrations.
- Task 12: all 296 original rows map to 521 unique identities; scoped runs passed
  501 parents, retaining 19 ignored protocol/model gates and one unavailable
  Windows-only identity. Isolated child entry points run through their owning
  parents with exact-selector and completion proof, not naked `--ignored` calls.
- Python: 76 discovered methods; 65 executed successfully and 11 native-Windows
  methods skipped. Both discovery roots and all four per-boundary commands pass.
  All 15 Windows CI selections load/run under `python -S`, with four host checks
  passing, eleven native skips, and no PyYAML import.
- The default source-unit selection passed 1,009 checks. Separately filtered
  VMAF/Vship fixture units passed seven/three checks. See [the runnable commands](README.md#rust-unit-only-selection).
- Fresh finite encoder/lifecycle fixture parents passed, including the stalled-
  tree watchdog. Required VMAF availability/library/model/clip/directory checks
  passed; required native/plugin CVVDP parity and its literal decode fixture passed.
  The post-review VMAF rerun recorded `--exact`, `--nocapture`,
  `AV_METRICS_REQUIRE_CPU=1`, both model paths and a 5,529,946-byte owned clip.
  Both checks passed without `UNEXECUTED`. An unset-environment control also
  reported two nominal passes, but printed `UNEXECUTED` and was rejected as
  required-native evidence. The final verifier checks the arguments, environment,
  clip size and absence of that marker, not just the pass count. These are
  prerequisite/availability checks, not blanket CPU-scoring acceptance.
- Linux CLI acceptance passed all 18 interrupt mode/route pairs, actual native
  ordered-frame/TQ propagation, partial restart, stale-identity refusal, terminal
  restoration and watchdog-counterexample rejection.
- Ten screenshots-feature source checks and one actual in-memory raster acceptance
  passed. On this host raster acceptance needs
  `CONDOR_SCREENSHOT_FONT=/usr/share/fonts/liberation/LiberationMono-Regular.ttf`;
  the default lookup misses Arch's font directory. No image generator ran.
- The active-consumer scan checked retired selectors and deleted/renamed paths.
  It found and repaired the installer guidance above; remaining leaf-name matches
  are current declarations/helpers, not stale selection commands.

## Limits and outstanding evidence

- **Offline schema acceptance did not pass:** its unchanged standalone command
  failed to link `OrtGetApiBase` because no native ONNX Runtime library was
  configured/cached for offline linking. Set `ORT_LIB_PATH` to a matching installed
  library directory (or provide pkg-config metadata), then run
  `python3 -B california-condor/tests/build/schema.py`. Its owned fixture cleaned
  up on failure. No network download or backend change was used to bypass it.
- Native Windows, macOS, Miri and hosted Actions were not executed. Source/selector
  checks do not establish Windows PowerShell, DLL loader or console acceptance.
  The known Windows QC wrapper/Unix-helper gating mismatch is preserved, not fixed.
- TransNetV2's model-required ignored gate was not executed. Native CPU fmetrics
  scoring and optional AVM/VVenC acceptance remain unverified. SVT grouped-zone
  parity reported `NoZoneSupport`; its required-capability negative control refused
  correctly, which is not positive SVT acceptance. Passing CVVDP parity is not
  blanket acceptance of every GPU/backend configuration.
- Two previously observed flakes remain unresolved: the SVT step resolver's
  missing capability-call file/optional-result mismatch, and inherited-lock
  configuration normalization reporting ENOENT. Isolated/full rechecks and final
  gates passed; this refactor does not claim to have fixed those flakes.
- Six pre-existing screenshots `unused_assignments` warnings remain in the
  optional feature build. Default precommit Clippy remains warning-clean.

## Fresh whole-branch review

**Assessment: ready. No Critical or Important findings.** One fresh-context
review covered `ad238cc8..3ee181d8`; no second review or production/test fix pass
was needed. The four original Minor findings have these dispositions: the VMAF
evidence-provenance concern was addressed by the explicit positive/negative runs
above; the remaining three are deferred below.

The reviewer used targeted passes over the 336-file, approximately 4.25 MB diff,
not a line-by-line read of every change. The passes checked production tokens,
original-to-mapped gate attributes, child selector/completion proof, private hooks,
the 1,009 source-unit selections, Python discovery/CI selections, consumer filters
and sampled names against assertions. The review found no lost assertion or gate;
all 68 original serial gates remain across 137 mapped cases, along with the
original panic/ignore and module-level platform gates.

The reviewer used source/grep and supplied artifacts, not graph queries, and ran
only the workflow integration selection (one pass) and script unit selection
(nine passes), not Cargo tests. The broader execution claims above are the
implementer's recorded checks, not independent reviewer execution. Native
Windows Rust tests were not type-checked on a Windows target. All eight behaviors
the reviewer declined to judge have explicit dispositions in rulings 97–104 below.

### Deferred minors

1. `andean-condor/src/core/process/windows.rs:243–260` retains a registered child
   route that returns when `PROCESS_LIFECYCLE_WINDOWS_ROOT` is absent. Count it
   only as a helper identity, not scenario coverage. Its parents use the descendant
   handshake; consider an ignored child role with a matching parent selector on a
   Windows host.
2. `.github/scripts/tests/integration/test_native_installers.py:408–508` silently
   ignores unknown Windows fetch-case names. The current four selections cover
   all 14 original cases; add membership/execution-count guards before extending
   them so a future typo cannot silently reduce coverage.
3. `california-condor/src/interrupt/tests/windows.rs:131–139` needs native
   confirmation that redirected stdout survives `FreeConsole`/`AllocConsole` and
   reaches the listener, retaining both exact-one summaries and completion markers.
   Linux source checks do not establish this Windows console behavior.

## Rulings I made

The following is the complete ledger ruling record, in execution order. It
retains each decision's rationale and cost if wrong; routine task logs are omitted.

1. Ruling: use the existing clean feature checkout, not a worktree — explicit user preference and appropriate branch — branch-only isolation.

2. Ruling: existing tests-only organization work is the REFACTOR phase with baseline/case/assertion parity; run true RED/GREEN for the five oracle defects — no production implementation is being invented — missed source moves remain guarded by exact behavior maps and full suites, not artificial filename tests.

3. Task 2: Ruling: correct that one existing expected fixture to `Some(QualityCheckConfig::default())` as a necessary consequence of the approved oracle repair — the current documented/code contract explicitly materializes defaults — if misread, a fixture could conceal an unintended default; full field comparisons now enforce it. This adds only the owning handler test file to Task2's commit, not a production change. Task12 retains the original row's rename/move ownership.

4. Task 3: Ruling: create only the currently used `current_encoding_fixture` helper now; add target/quality helpers at their first actual migrated consumers — avoids unused future scaffolding/lint suppression — later tasks must retain the planned exact signatures.

5. Task 3: Ruling: name the first force case `forced_mux_cancellation_does_not_finish_or_publish_output`, not a process-reaping claim — this scenario checks cancellation/finish/publication, while the separate Linux matrix checks owned PID reaping — if renamed inaccurately, perceived coverage would exceed the actual oracle.

6. Task 3: Ruling: share only test fixture setup between public final-preflight and private prepared-source checks, not the assertions or production module — public API calls cannot inspect prepared source tags before final mux — if misclassified, the private staging regression could disappear despite passing final-output checks.

7. Task 3: Ruling: external supervisors use public `OwnedProcess::wait` plus forced cancellation on the same20s deadline instead of exposing crate-private `try_wait` — preserves tree ownership/reaping and API visibility — a wrong adapter could mask hangs, so one real-process normal/forced check remains, observed RED for suppressed timeout reporting then GREEN (`task3-watchdog-{red,green}.log`). The shared helper is registered once per target, avoiding Clippy duplicate_mod; no production API is added.

8. Task 3: Ruling: source-side private native observation keeps its necessary successful drive while public ordering/provenance/completion cases exercise their own boundary without the hook — assertions differ and cannot be substituted — costs additional tiny native encodes, not missing private proof.

9. Task 4: Ruling: keep pure bare-name/empty policy branches as units while splitting their original mixed host-path registrations — they never consult the filesystem — misclassification would unnecessarily place pure coverage behind the internal-integration exclusion. No production loader behavior changes.

10. Task 4: Ruling: consolidate old `auto_feature_policy`, `cpu_only_feature_policy`, `require_cuda_feature_policy` into the retained named stage0/7/9 cases — their setup, explicit partial=false mode, preference, child and `check_attempt` assertions are identical to the full45-tuple matrix — source row mapping preserves all nine outcomes instead of running them twice.

11. Task 4: Ruling: use one local registration macro for57literal named constructor cases — avoids repeating the same process wrapper57times while each Cargo identity remains individually selectable and the full table remains source-visible — macro-generated discoveries must be reconciled against Cargo listings, not counted as one template test. This is test registration only, not a new framework/dependency.

12. Task 4: Ruling: move the assertion-free synthetic exporter into a manual Cargo example, explicitly test=false — audit requires removing automated annotation while preserving export; test=false avoids duplicate sharedfixture checks — extra example/manifest entry buys a runnable manual command without claiming scoring proof. Output byte-identical to prior12-frameexport (`task4-manual-export.log`), generated export files removed.

13. Task 5: Ruling: lifecycle reentry/poison tests retain earlier successful native phases as setup — originalfinal11handlefreemask and one-default-probe assertions depend on those phases — costs extra smallCdoublechildren, not weaker privateownership proof. Newconcurrent/callbackcases stop only after their own originalassertions; poisoncase still reaches originalfinalvector11/11,0live,0x7ffmask.

14. Task 5: Ruling: use12literaldiscoverymacroregistrations over the retainedcasebuilder — keeps allplatform-specific orderingtuples source-visible without12copiesofsetup — verifyactualCargolist and labelmapping, not the macrotemplatecount. FullTask5stillpending; firstsubsetprecommit/commitnext.

15. Task 5: Ruling: result-contract children stop after their own phase while later error contracts retain prior successful phases as setup — preserves the original exact5/5/16Cvector and nativeprobelifetime — costs repeatedsmallCdoublecalls. New intermediate4/4/4and5/5/10counterchecks additionally protect phase cleanup; no original predicate removed.

16. Task 5: Ruling: keep GPUuploadfixture sensitivity unit beside its localgenerator underexplicitfixture_unit rather thaninventingproductionmodule — it tests only syntheticpadding andnotascore — fixture-specificexception tosource-ownedproductionunits, registeredonce. Generalsharedgeneratorchecks remaininthesupporttarget.

17. Task 5: Ruling: keep successfulprefixphases inlatercapability/version contracts to retain exactcache/probe/ownershipvectors — independent names need distinctendconditions, not weaker resetcounterfixtures — extra boundedCdoublecalls, no newproductionseam. Matching/wrongtags are partitionedpermetricbeforeconstructor, retainingall69oldoutcomes, with explicitapplicablemetriccount checks.

18. Task 5: Ruling: no additional synthetic-dump command is invented inthese2packages — their211auditedrows contain no automatedexporter; Task4convertedtheactualVMAFexporter andVshipBENCH_DUMP_FRAMESalreadybelongs tomanualexample — leavingtheexistingbenchhelperuntouched avoids unauthorizedbenchmarkexecution or speculativefeatures.

19. Task 6: Ruling: remove temporarynowemptyvapoursynthparserharness now; recreate registeredpluginroot only when actual publicpluginconsumers are moved — no emptytarget/compatforwarder — if missed later, finalregistrationmapmustcatchit. Miriactualmanifest isrepo-root tests/miri-grain/Cargo.toml, notnestedunderAndean; preservethatpath.

20. Task 6: Ruling: retain the singleCVVDP inline testmodule, addingexplicit tests::internal_integration ratherthannewphysicalfile — owningtestmoduleandcfg(test)boundaryunchanged, all4contracts visible, noAPIexposure — ifphysicalfilelayoutisrequiredlater, movemodulewithoutbehaviorchanges. Rawheaderoracle alreadylivesinraw/tests.rs.

21. Task 7: Ruling: row293formatter is aunit, not publicintegration — sourcebaselinealreadyusesUnixExitStatusExt::from_raw for42/0/Noneandspawnsonothing — keepitbesidepipesunchanged; iflaterimplementationaddsnativeI/O,reclassifythen. Thisoverridestheaudit'slocation-basedcandidate, nottheassertions.

22. Task 7: Ruling: map olddispatcherrows341/343tothebehavior-namedself-reexecutingtests andretainedhelpers, notnewgenericrun_isolated_*registrations — normalrunsmustexerciseactualoutcomes — registrationcountdrops2whilesplitfailureparentadds2; assertions/caseaxesaretheproof.

23. Task 7: Ruling: row348 isunitnotpublicintegration — originalassertionsonlyinspectargvfor7pathoverridesand2manualVVenCflags,neverfiles/deletion — keepbesidesourcewithhonestnative_statistics_path_options_override_generated_defaultsname; actualcallerfilepreservationremainsinpublicmanualworkflow. Costifwrong: boundarylabelonly, originalassertionsunchanged.

24. Task 7: Ruling: consolidateoriginalstdin-fullduplicate(row337)withthesamecasefrom59selectorparent(row336) — bothcompiledthesameimmutablefixture,selectedthesamechild/helperandcaseenvironment,andassertedthesamerescued0/selectedsuccess; stricterexact1testnowappliestobothmappedidentities. Fulloriginalchildrawbytes/producerjoin/typederror/cleanupassertionsremain. Costifwrong: losswouldonlyaffectadifferentroot/symlinksetup; retainedcanonicalfixtureprogramandall59caseproofsmitigatethat.

25. Task 8: Ruling: retaincumulativeinvalid/raw/typedsetupinlateruploadphases,andfinitebaselineinButterauglinonfinitephase — exactsubmitted/calllogsand2callbacks/10create/10destroy/5scorevectorsdependonthoseprecedingoperations — costifwrong: extra repeatedfixturework,notlostcoverage;alloriginalpredicatesretained.

26. Task 8: Ruling: graphreplacement/independencecasesretaincumulativeoriginalbaselineandmodifierphases — literalbulkimagesandpriorgraphcount1oracledependonthatsetup — costifwrong: repeatedtinygraphwork,notmissingassertions;wholeoriginalbodyreconciledafterexplicitphaseguards.

27. Task 8: Ruling: namechanging-resolutioncapability`ffms2_resolution_changes_reject_exposed_drift_but_accept_fixed_output` — actualnativecanrefusechangesorholdoutputfixed,soassertionrequiresrejectiononlyifexposed — costifwrong: namecouldoverstatehostcapability; retainedexplicitNO-rejectionloggingpreventsapassing-capabilityclaim.

28. Task 8: Ruling: row493mixedoriginalsplitsonepureengine_order(false,allavailable)unitandpublicengine(true/false)runtimefallback — noneedsprivateintegrationafterseparation; unitnameavoidsany-inputclaimbecauseoriginalpurepredicateonlycoverednativeinput — costifwrong: classification/nameonly,all3originalpredicatesretained.

29. Task 9: Ruling: row238sharedfiltereligibilitystayspureunit — originalonlyconstructsmodels/callsZonePlan::try_build,nofilterexecution — costifwrong: overstatedunitpurity; actualnativefilter/encodingchecksremainseparate. Format-onlyx265/SVTnamesnow"format"ratherthan"write".

30. Task 9: Ruling: middlecreditphaseisretry_commit_reconciles_missing_frames,not"completes_once" — completioncount3isassertedafterzero-phaseinoriginalcumulativebody — costifwrong: lesssearchableidempotenceclaim,notlostcoverage; finalphasekeepsrepeatedcommit/countoracleandwholebodyreconciliation.

31. Task 9: Ruling: force/force-child-firstcasesstayinternal — externaltrialfailedatmissingknownleader-reapsignal;core/process.rsrecordsitonlyundercfg(test,Linux),sopublicAPIcannotproveexactleaderSIGKILL — costifwrong: fewerpublicforcedworkflowregistrations,notlostcoverage;oldassertionandbothinputvariantsremainintactinternally. Publicnamesnarrowtograceful/existingcancellationandfailedpublicationratherthanoverclaimforceproof.

32. Task 9: Ruling: useonesharedtest-onlyruntimefixtureforparityandfailed-sequences;sourceincludesitwithlocalizeddead_codeallowfor3public-onlyparityhelpers — avoidsduplicatingcommonimports/fixturesorintroducingproductionAPIs — costifwrong: unusedfixturefunctionscompiledinlibtestsandunusedhelperdriftmaynotwarnthere;externalClippychecksremainunsuppressedandexecuteparity.

33. Task 9: Ruling: narrowTQpolicycontracttoindependent_scene_probesratherthanclaimactualzoneddispatch — originaltq-zone-*isstructurallyeligiblex264butassertsexactly2independentencodes — costifwrong: lesszone-relatedsearchability,notlost4-scopecoverage;actualzonepublicvalidationispreservedinthepreflight/ratecontracts.

34. Task 10: Ruling: row206nameis`probe_encoder_without_an_ffmpeg_filter_stays_unfiltered` — originalfixtureclearsfilterbeforecallingremove_psychovisual_parametersandthereforecannotprovefilterremoval — costifwrong: lesssearchabilityforfilter-removalintent;noassertionremovedandnounapprovedoracle/productrepairadded.

35. Task 10: Ruling: auditrows71/72aremixed,notpublic-only — compileprovedSceneScoringStartedispub(crate),andbothoriginalworkflowsassertitsdowncast — retain2sourceinternaldriverduplicateswithoriginalprivateassertionsviatest-onlyoptionalcallback;publicversionsstillassertactual3-framepartialprogress/noattachment/save/completionorresetatomicity — costifwrong:6additionalisolatednativechildrenandsharedcallbackplumbing,notAPIexposureorcoverageweakening. Publichistorysplitsadd4scopedexecutionssobothnamedoutcomeskeepthecompleteoriginalmatrix.

36. Task 10: Ruling: consolidateidenticalrepeatedcancellation/admission/gapphasesfromoldfinal-reductionparentwiththeirnamedconsumercontracts — botholdparentscalledthesamenative_consumers(engine)withthesame4scope/failure-emptyenvironmentpairsandnoadditionalinputs — costifwrong: lessredundantfixturework,notdistinctscenario/assertionloss. Separatetemporal-final-prefixandindependent-statisticreductioncontractsretainthosesamehelpersphasesagainforselectability;alloriginalbodytokensandtyped/logoraclesreconciled.

37. Task 10: Ruling: complete-reusenameclaimsnoscratchguarantee — originalfixturecreatespassdirectoriesanddoesnotremovethem — use`complete_probe_pass_reuses_valid_records_when_source_outputs_and_encoder_are_missing` — costifwrong: lesssearchabilityfororiginalscratchintent,notlostassertions;synthetichistorycommentremainsconsumer-only.

38. Task 10: Ruling: row174 also depends on private`quantizer_score_history`, discovered by external compilation — share its drive with a supplied reducer; public cases use public metric reduction, one internal phase1 drive retains both exact private reducer calls on real measured records — costifwrong: one extra native drive and a test-only function parameter, noAPI exposure. Later max/force phases retain original cumulative setup and save-count baselines.

39. Task 10: Ruling: row195 has no failed-save injection and cancellation checks only typed precedence after prior restoration — name its split outcomes restoration/typed cancellation/disabled-or-empty no-work, not the audit's proposed failed-restoration-save guarantee — costifwrong: less searchability for an unsupported original intent; no oracle dropped or invented.

40. Task 11: Ruling: consolidate time-display rows980/985, both literally`seconds_to_hms(5,false)=="00:05"`, into one named subminute-duration contract — same inputs/output/assertion and no distinct setup — costifwrong: less redundant registration, not different behavior coverage. All other duration values and forced-hour control retained; both original rows map to the one case.

41. Task 11: Ruling: narrowdry-runname to`zone_dry_run_flag_parses_around_negative_bound_definitions` — originaloracleallowszones.len()<=1andthereforecannotprovea suppliedzonewasnotswallowed — costifwrong: lessassertedcoverageinname,notlostassertionsorneworacle. No product/parser behavior changed.

42. Task 11: Ruling: narrowselected-writererrorname toreturningwrite/flusherrors,notno-retry-on-other-stream — original4tuplesassertreturnedcausebutdonotobserveanotherstream'sattempts — costifwrong: lesssearchabilityforintendedstreamownership,notlostassertions;actualPTYownershipcoverageispreservedelsewhere.

43. Task 11: Ruling: repaironlythose2testfieldpaths to`scene.bytes`soexistingpositive/NoneassertionscompileagainsttheactualSceneDisplayprojection — thisisfixture/typewiring,notaneworacleorproductbehavior — costifwrong: byteeligibilitywouldbetestedatdisplayprojectionratherthanrawrecord,whichisthecurrentfixture'sactualstate. Preservedredfeaturediagnostics`task11-render-route-build-red.jsonl`;greenfeaturebuildretains6preexistingproductionunused-assignmentwarningsat279–295,notsuppressed/fixed.

44. Task 11: Ruling: rows1175/1178/1274arehybridpublicfilesystemintegrationcontracts,notpureunitsasinitiallyclassified — keep9behavior-namedcasesundertheexistingexternalcommandsowner,becauseexplicitread_dir==0and!configuration.temp.existsoraclesprovefilesystemeffects,whilethespecrequirespublicintegrationstostayexternal — costifwrong: threeoriginalrowsarenotmovedbesideproductionandtheirpuremutationassertionsremainwithincheapintegrationdrives;noassertionsdropped,noduplicatepurefixtureframeworkorAPIexposure. Thisoverridesbrief'seight-pure-externalassumption;fiveoriginalpureexternalrowsmovedtosourceintotal.

45. Task 12: Ruling: newutilitytargetusesownedfinite2-frame32x32FFmpegmediafornativevideo-only/multitrackcontrols,insteadofsharedfixedcachedclipnames — movingtonewtestprocesswouldotherwiseintroducemorecross-targetfixtureinitializationraces — costifwrong: no30s/1080pinputexercisedbythesetwoidentitychecks,buttheoriginalassertionsonlycovertrackIDs/indices/counts;alloriginalnativeassertionsremainandrealFFmpeg/mkvmerge/ffprobeboundariesstillrun. TempDirsurvivesthroughbothFFmpegcommandsandnativeprobes;nosignaturesofpublictesthelperschanged. NativeY4Mfixtureandassertionsunchanged.

46. Task 12: Ruling: keepLinuxinheritance/pre-execunlock/successor/ordinary-crashcontrolsasonecoherentlifecyclewithname`inherited_lock_drop_unlocks_before_exec_preserves_successor_and_crash_releases_owner` — thetesttracksonepersistentinodeandtheexactsamefork-inheriteddescriptionthroughDrop/exec/successorownershipandthencrashrelease;partitioningwithfreshfixtureswouldlosethatidentitychain,whileearlyreturnwouldneedkill/releasechanges — costifwrong: coarserindividualoutcomeselection;alloriginalnativeFD/proc/FLOCK/CLOEXEC/Pending/Reap/barrier/timeout/childroleassertionsremainunchanged. ExactTESTconstantupdatedatomically. EarlierCaliforniaflakeisstillunresolvedunderthisrenamedbody,nofixclaimed.

47. Task 12: Ruling: share the canonical cfg(test)-only quality fixture and public-API resume drive between the external and private harnesses — private raw state cannot be observed publicly, while a duplicated fixture would drift — cost if wrong: two cheap model/event drives instead of one; no native pipeline, production API or public test-helper signature added. The old cfg(test) fixture path stays re-exported. Every other helper/test body remains unchanged.

48. Task 12: Ruling: external prefix-report contracts complete the same saved-history model via public cached_state_mut and invoke public after_loop, while source checks retain direct private print_report().expect success — private report result cannot be observed externally, and the original fixture includes an unfinished unmeasured scene that suppresses after_loop — cost if wrong: public completion is arranged rather than proving natural completion; original direct-report success and stdout predicates remain covered, not replaced by a weaker public drive.

49. Task 12: Ruling: use one local registration macro for7literal parent/child pairs, retaining Unix+ignore gates on every controlled-PTY entry — avoids fourteen repeated wrappers without hiding operation tables or weakening ignored child isolation — cost if wrong: generated registration discovery requires Cargo-list reconciliation, and ignored count rises by6; all14actual identities were listed and all14children were explicitly served by parents. This is registration plumbing, not a new scenario framework.

50. Task 12: Ruling: use a local10-entry registration macro with one named test serving its own isolated child — no dedicated no-op child registration and no parent subscriber mutation — cost if wrong: generated names need actual-list and selector reconciliation; all10were listed and all18childtuples completed with matching markers. First Clippy run rejected the semantically valid `test_mode_` prefix; renamed atomically to`console_test_mode_`,retaining redlog without suppressing the lint. Broad app internal run passed23parents/9ignored plus all served children;full `just precommit`:2338reportedpasses/0failures/11ignored;diffcheckpassed.65/296mapped;231remainingrows,30panic/105finish-stage and remaining public/native acceptance pending.

51. Task 12: Ruling: retain startup retirement/raw0/0 and subsequent writerless cleanup/raw1/1 as one named cumulative control — both assert one shared raw-counter/hook/owned-process lifecycle; splitting would duplicate the intentional startup panic or change counter baselines — cost if wrong: coarser selection for writerless cleanup; the name explicitly states both outcomes and every original assertion survives.

52. Task 12: Ruling: give the original Python stale-identity/later-owned-cleanup counterexample its own named source supervisor and keep12panic registrations in one local literal macro — prevents repeating the5s safety counterexample12times while preserving it once per complete suite — cost if wrong: selecting one panic outcome alone no longer also runs the cleanup counterexample; full suite includes the dedicated check and actual-list/selector reconciliation covers all13registrations. Subreaper/pidfd identity+safe signaling,watchdogs,error cleanup and source/output sentinel checks remain unchanged.

53. Task 12: Ruling: split the3execution outcomes across successful-display/I/O/panic contracts and retain str/String/opaque panic forms as a homogeneous3-value table; use a local9-entry registration macro and retain ignored child gates — each parent names an independent finalization contract without105boilerplate functions — cost if wrong: individual panic payload forms remain table-selected rather than separately registered; actual Cargo identities and all105tuple-completion records were reconciled. Ignored count rises by8 because9controlled children replace1,not because coverage was disabled.

54. Task 12: Ruling: narrow custom speed case to saving requested pairs,not sorting,and refusal names to checked saved fields,not bytewise file identity — original input10/25/35 is already ascending and check_basic_config is a field comparison,while noise scene oracle only proves nonempty — cost if wrong: less searchability for intended sorting/full-scene guarantees; no assertions dropped or new unapproved oracle added. The copied historical inline CWD block comment was removed; mapper normalizes only that comment,not arguments or assertions.

55. Task 12: Ruling: keep no-op/creation beside source as genuine units and retain the full cumulative original no-op→creation→edit drive externally for the no-scratch oracle — only final absence assertion is a filesystem contract,and that cannot be dropped to force all outcomes into one taxonomy — cost if wrong: duplicated cheap constructor/prefix assertions,not duplicate native pipelines or weakened outside-section/absence coverage. All remaining source helpers/tests and prior external consumers unchanged.

56. Task 12: Ruling: retain the native row's cumulative fixture/probe baseline in its three selected phases — preserves the original pre/post-method-change snapshots and exercises actual native identification,not a substituted fake — cost if wrong: repeated cheap identification in later selected phases; no repeated full pipeline or lost native requirement.

57. Task 12: Ruling: keep save_edit checks internal and retain failed-publication setup for later validation/success cases — pub(super) staging seam is not publicly callable,and successful proof restoration depends on the same prior lifecycle/snapshot — cost if wrong: repeated native construction/scaling setup for the two proof outcomes; no API exposure or duplicate complete encoding pipeline. Source inspected alongside graph trace; no reliance on graph's unrelated clone resolution.

58. Task 12: Ruling: retain global edit as cumulative setup in the private scene contract while adding a cheap public-only persistence drive — private scene edits/rendering are unavailable externally and should not become production APIs — cost if wrong: duplicate cheap model/save setup,not duplicate native pipelines;all original source assertions remain intact.

59. Task 12: Ruling: move private validate_encoder_settings zero-budget predicate into an owning model-only Configuration unit rather than widen the API — initial external build exposed its pub(crate) boundary;the method first validates the same max_tries field before global/per-scene settings — cost if wrong: the direct predicate no longer shares the cumulative CRUD fixture;public instantiate/save/handler refusals retain the entire original deleted-source baseline. Compile-failure log retained. A separate Clippy finding required cfg(test) on the registered retry_budget module;fixed without suppressing the lint.

60. Task 12: Ruling: retain cumulative setup across6public phases and narrow zero-refusal name to unchanged saved bytes — earlier start has a valid native clip and its original zero call alone does not prove unopened-source ordering;later deleted-source cases retain that proof — cost if wrong: repeated cheap metadata/CRUD setup,not full native pipelines;name no longer overclaims both handlers' earlyIO guarantee.

61. Task 12: Ruling: keep supervisor/listener/fixture/leaf as unregistered roles of one named self-reexecuting test — these are process-tree roles sharing an OS console and exact owned lifetimes,not independent outcomes,and a separate ordinary child registration previously returned without checking anything — cost if wrong: coarser role selection;no coverage disabled,additional exact-one/completion checks require both observing roles. NativeWindows compile/runtime/listings remain UNVERIFIED on Linux;source mapping is not Windows acceptance.

62. Task 12: Ruling: keep only missing-main-input case forrow800,not the audit's proposed3-way split — original SCD/TQ missing-input calls and assertions were commented TODOs,not executable coverage — cost if wrong: those two intended behaviors remain untested;reviving them would add unapproved oracles and overclaim preserved coverage. Removed obsolete commented calls,retained every actual assertion.

63. Task 12: Ruling: consolidate the two byte-for-token-identical creation prefixes into one local helper with an early return for new-job outcome — both original tests already repeated precisely the same construction and every assertion;fulloriginalcreation and update bodies each reconstruct exactly — cost if wrong: sharedfixture edits affect both cases,guarded by retained per-original body maps. This removes duplicated test code,not an outcome or assertion.

64. Task 12: Ruling: keep scalar setup+invalid101control as cumulative baseline for reset and expose range-refusal separately — original persisted8/8192 assertions follow refusal,so omitting that control would lose atomic scalar retention evidence — cost if wrong: duplicate cheap CLI CRUD setup,not duplicate measured pipeline.

65. Task 12: Ruling: name final reset as restoring one filter,not the default list's contents — original predicate only checkslen1;theclearwordcase also follows an alreadyemptylist — cost if wrong: less guarantee than the old prose implied,not less coverage;no new oracle beyond approved repairs. Later CRUD keeps disabled-reset baseline and both empty-list/none calls intact.

66. Task 12: Ruling: retain expensive row1236 encode→concatenate→initialQC→populated-history-rerun asone combined behavior name — independently arranging two registrations would duplicate the complete native pipeline or add shared fixture globals — cost if wrong: first/rerun count outcomes cannot be filtered separately;name states bothandalloriginalassertions survive. Old partial-resume/sentinel-preservation comments were unsupported:everyhistorywasalready populated and finaloracle only checkslen11;names/comments now state that exact ceiling.

67. Task 12: Ruling: expose plugin and CRUDprefix outcomes while keeping their cumulative final controls — same two destination routes and changed score_order prove comparison/admission,while final CRUDreset follows saved metric/statistic changes — cost if wrong: repeated cheap2frame/plugin or CLIsetup acrossselectedtests;no duplicate full encoding pipeline. Other originalsource/externalfunctions unchanged.

68. Task 12: Ruling: keep algorithm transitions and inherited/rebuiltwarningroutes cumulative/homogeneous — they test the same carried settings or warningpolicy across adjacent variants;no duplicate native detection pipelines needed — cost if wrong: per-transition filtering remains coarser;every original assertion and name's actual guarantee retained.

69. Task 12: Ruling: keep resized encoding and both concatenators asone explicitly combined outcome — originaloneencode supplies bothhomogeneousmethodchecks;independenttests would duplicate the expensive completepipeline or require fixtureglobals — cost if wrong: filter cannot choose one concatenator here;dedicatedconcatenationcoverageexistselsewhere. Name says scene-countfiles/outputexistence,not chunkidentity or decodedfinalpayloads that these assertions do not prove.

70. Task 12: Ruling: retain cumulative saved-knot/refusal setup for quantizer-only andrepair/reset outcomes — original later snapshots and no-source/scratch predicates depend on those mutations — cost if wrong: duplicated cheap filesystem/CLI setup,not native pipelines. No claim of per-scene assigned speed/preset beyond the existing count oracle.

71. Task 12: Ruling: retain homogeneous4unsafeoutput variants in a table and cumulative refusal/consent/CRUD setup — contracts differ by consent/property,not every alias spelling;later snapshots depend on earlier writes — cost if wrong: aliasvariant filtering coarser,cheaprefusalsetup repeated,not nativepipelines.

72. Task 12: Ruling: narrow FFmpegkeep case to two dropped andatleastonekeptstream — originalany(copy) doesnotproveeveryotherstreamorstreamidentity — cost if wrong: weakername than intended policy,not weakercoverage;allactualassertions retained without newunapprovedoracle.

73. Task 12: Ruling: retain extra-video selection and new-attachment byte/metadata checks together in the same two native muxes — separating them repeats the complete native construction for no stronger oracle — cost if wrong: those coupled output predicates cannot be selected independently;the separate extraaudio/attachment selection remains filterable. Later controls retain cumulative before/after output and source baselines;no production API or native gate changed.

74. Task 12: Ruling: keep cumulative known-producer snapshots and edits for later recovery/population outcomes — exact baseline/provenance and no-second-scaling proofs depend on the same saved mutations;individual named phases now select each meaningful result — cost if wrong: repeated metadata/CLI setup across selected phases,not duplicate native encoding pipelines. New-population recovery stays coupled to withdrawal for both original edit routes;no new detector or score oracle.

75. Task 12: Ruling: preserve cumulative saved producer/revision/callback state in later zoning outcomes — direct/CLI reruns and failure controls assert the same exact baseline or savecounter,not independent blankjobs — cost if wrong: repeated cheap scene/model/native-metadata setup;no complete encode pipeline duplicated. Homogeneous definitionformats/edit routes stay readable tables and all serial/native gates remain.

76. Task 12: Ruling: narrow mismatched-encoder name to reporting matchedscenes unchanged andtwo warnings,and dry-run preservation to job/chunks — original mismatch only asserts logs,not savedencoders,and preview doesnot inventory everyfile — cost if wrong: name acknowledges missing broader coverage;no assertion weakened or unapproved oracle added.

77. Task 12: Ruling: keep public-entry invalid-configuration and empty-scene admission matrices homogeneous — each checks the same rejection/acceptance boundary across public routes and failure classes — cost if wrong: individual entry filtering remains coarser,not lost behavior. Runtime still constructs nativeVapourSynthBlankClip;emptyPATH execution is not proof of native-free units. Cumulative persistence/repair snapshots remain in later outcomes.

78. Task 12: Ruling: configured targetless handler name claims savedmetric/history/output only,not unchangedtargetrange — original assertions do not comparetargetrange — cost if wrong: missing broader targetcoverage remains visible;no unapproved oracle added. Probeparams owns the existing full statistic+param workflow;statistic-only registration omits params operations while retaining every original statistic predicate.

79. Task 12: Ruling: move legacy-output JSON and schema default/type predicates from mixed overwrite workflow into an owning model-only Configuration unit — filesystem/native preparation contributed no asserted behavior to those predicates — cost if wrong: unit uses an equivalent trueoverwrite model rather than the prior loaded job;original CLI rootflag true is still asserted and full pure block matches after type/inputalias substitution. No source/test API or production code widened.

80. Task 12: Ruling: keep overwrite revocation and source-root mutation variants cumulative/homogeneous — assertions depend on saved consent and scene-state snapshots;later reset/detection refusals preserve the same job — cost if wrong: coarser filtering per variant and repeated cheap nativeVapourSynth metadata,not duplicate full pipelines. Existing custompaths check doesnot claim customlog file contents;CVVDP checks remain configuration-only.

81. Task 12: Ruling: relocate the build-only positive subset percentage table(MIN_POSITIVE,25,100) to commands/config owning unit while keeping invalid persistent edit routes external — disk/native preparation is irrelevant to the acceptance predicate — cost if wrong: the equivalent original StrategyArgs fixture is now model-only,not a loadedjob;the exact original args+loop/assertions match. Invalid build predicates remain alongside their real public persistence checks;no oracle lost.

82. Task 12: Ruling: retain the actual current-producer lifecycle asone explicitly combined native case instead of seven proposed registrations — every later control depends on certificates from the same real TQ/encode/QC pipeline;separate arrangements duplicate that entire pipeline or introduce mutable fixtureglobals — cost if wrong: individual native edit/barrier phases cannot be filtered separately;all8variants/exactcall/file/proof assertions remain. This is not synthetic provenance or an unexecuted native claim.

83. Task 12: Ruling: narrow scene encoder name to selectedX265CRF,subscene name to two subranges,and report name to assertedsavedresults/subscenes — original checks do not compare allpeers/fullserializedreport/everyrange — cost if wrong: missing broader coverage is visible;no oracle weakened. Later phases preserve cumulative frame mutations,loaded state and explicitreset baselines;no newassertions or production APIs.

84. Task 12: Ruling: move the two argv-builder checks and zero-attempt candidate predicate into encode's owning source units — tempfile allocation was incidental,not the asserted boundary — cost if wrong: model-only paths replace temporary directories;exact ClipInfo/24parameter tables/assertions and equivalent full input+metric+QC fixture match by tokens. No production API exposed;all prior sourcefunctions/production prefix unchanged. External mixed case retains its original directory-count guard after pure predicate extraction.

85. Task 12: Ruling: keep real rebased encode+TQinitialization in one explicitly combined lifecycle — splitting would repeat the native encode or replace its established state with a different oracle — cost if wrong: initializer phase not separately selectable;actual encode/progress/directory assertions remain and no quality probe run claimed.

86. Task 12: Ruling: use inline owner modules in the existing registered source-mirrored config file rather than create10one-off files — owner boundaries are now filterable without extra registrations or fixture visibility APIs — cost if wrong: source remains large;separate files can be introduced later if navigation proves insufficient. Original module-qualified filters changed;no live source consumer of handlers::config::tests found,Task14 exhaustive consumer check still pending.

87. Task 12: Ruling: keep format variants/statistic/executable edit baseline cumulative andhomogeneous — later refusals depend on the established savedVS/native state or prior value — cost if wrong: repeated cheap seed/CLI metadata setup;no native decode/encode pipeline duplicated. Alreadydescriptive persistence/offline names stay unchanged after review,not mechanically prefixed.

88. Task 12: Ruling: keep one complete owning native pipeline per inherited/explicit probe mode instead of3/4proposed registrations — all downstream counter/history/base/rollback assertions rely on the same actual probe+encode producer state — cost if wrong: individual downstream native phases are not filterable;no pipeline duplicated or certificate substituted. Behavioral names describe the primary end-to-end contract while supporting negative controls remain.

89. Task 12: Ruling: earlier selected lock outcomes let existing JobChild RAII kill/reap the paused holder;crash outcome retains explicitkill/wait/recovery — controlled45sscript and30smarker/5scontender watchdogs remain unchanged — cost if wrong: selected earlycases do not exercise postcrash recovery,which has its own fullphase. Every original statement reconstructs and nofixture outlives its child.

90. Task 12: Ruling: keep homogeneous alias/decoder/route matrices together and keep handler save+custom-directory diagnostics coupled — they assert the same per-fixture boundary;separate benchmark-handler registration skips CLI source failures,not actual benchmarkexecution — cost if wrong: per-variant filtering remains coarse;all original producer→savedstate→finaldirectory controls retained,including cheap handler call in the CLI case.

91. Task 12: Ruling: permission-case names explicitly allow accessible-path policy fallback — chmod000 may not deny access;actual operation probes,PermissionDenied/context assertions,700restoration before assertions/cleanup and unavailable-proof diagnostics remain unchanged — cost if wrong: no blanket denial-acceptance claim;environments bypassing permissions still verify nonempty-work or allowed-source policy.

92. Task 12: Ruling: keep real detect→zone→TQ→encode→concat→QC and native interruption/restart/counterexample inone descriptive acceptance registration — PTY jobs deliberately consume actual TQ-produced encoders and restart controls freeze that same partialproducer's bytes/mtime;independent registrations would repeat a full native pipeline or weaken the producer oracle — cost if wrong: individual native phases not separately selectable;all literal16frameorder/±3/VMAF98..100/provenance/3chunks/exactunfinished2encodes/staleidentity/watchdog rejection proofs executed.

93. Screenshot10feature checks passed with6preexisting unused_assignments warnings. Initial raster testfailed solelybecause defaultfontsearch omits Archfontpaths;existing `CONDOR_SCREENSHOT_FONT=/usr/share/fonts/liberation/LiberationMono-Regular.ttf` made1/1actual in-memory raster acceptance pass. No screenshot generator or outputimagefiles run. Task 12: Ruling: use supportedfontenv,not change productionfontlookup — environment prerequisite,not naming/boundary scope — cost if wrong: defaultfontlookup limitation remains and mustbe disclosed.

94. Task 12: Ruling: count preserved ignored protocolentrypoints separately from available parent passes — parents arrange their required ownership/env and verify exactchild+completion,whereas naked `--ignored` would not exercise the same contract — cost if wrong: parent/child proof,not rawignoredtestcount,must remain part of final audit. No ignored/platform gates weakened. Whole-taskcompletion command follows.

95. Task 13: Ruling: retain nativePowerShell scratch/retry/collision protocol asone explicitcombinedcase andcandidate/PSDrive/download protocol asone descriptivecase ratherthan3/8proposedregistrations — theirsame-interpreter location/env/drive/finally andowned-scratch controls formexistingvalidatednativeprotocols;newWindows-onlybrancheswithoutanativehostwouldmakepreservationweaker — costifwrong: thoseWindows subphasesremaincoarselyselectable;entireoriginalscriptandallnegativecontrolsareunchanged,15requiredCIselectionsretainalloriginalnativebehavior. NativefetchmatrixsplitusesunchangedPSrunnerandverifiedcompletecasepartition,notnewPSprotocols.

96. Task 13: Ruling: preserve cumulativeGit/publication snapshots acrosslateroutcomes — stale-run/tag/bodyandouter-repositorychecksdependonactualearliermutations — costifwrong: repeatedcheapownedGitsetup,notrealnetwork/nativebuilds. Namedphasesretainalloriginalstatementsandcleanup;nofakecertificateorremotecredentialbehavior substituted.

97. Final: Ruling: retain internal retry replays of public cases in `andean-condor/src/core/sequence/parallel_encoder/tests/retry_progress.rs:66–131` — the source build executes private `after_failure` cleanup assertions that the external target cannot observe, so users retain both public behavior and private cleanup proof — cost if wrong: repeated fixture/runtime cost, not lost coverage or a widened API.

98. Final: Ruling: retain the deliberately coupled pipelines in rulings 46, 51, 66, 69, 73, 82, 85, 88, 92 and 95 — shared producer, identity and lifecycle chains preserve the original assertions without mutable global fixtures or repeating complete native pipelines — cost if wrong: individual phases remain coarsely filterable; their stated scenario and ownership checks must stay intact.

99. Final: Ruling: keep environment assertions on the 18 ignored California child entries even though naked `--include-ignored` fails — ordinary runs use their owning parents, which arrange isolation and check exact selection/completion; an unarranged child should fail rather than report vacuous success, and the baseline already required a model for an unfiltered ignored run — cost if wrong: ad hoc direct-child commands require their documented parent protocol; no supported all-ignored success claim is made.

100. Final: Ruling: keep CUDA-stage constructor names with their feature-aware assertions in `av-metrics-vmaf/src/scorer/tests/setup.rs` — default builds check CPU fallback without entering CUDA, while the CUDA-feature build exercises the named injected stage; `just precommit` and the README retain both configurations — cost if wrong: a default-only pass could be mistaken for CUDA-stage or hardware proof, so neither is claimed here.

101. Final: Ruling: keep the integration target named `core` in `andean-condor/tests/core.rs` despite shadowing the standard `core::` namespace — it is the approved source-mirrored target and explicit imports compile against the intended owner — cost if wrong: future ambiguous paths cause compile errors and need qualification; no runtime behavior is changed.

102. Final: Ruling: retain the optional prerequisite gate's environment-absent success in ordinary VMAF suites — it explicitly prints `UNEXECUTED`, while required CI sets `AV_METRICS_REQUIRE_CPU=1`; the refreshed evidence and verifier now distinguish nominal success from executed prerequisite checks — cost if wrong: an ordinary pass counter remains easy to overread, so it is not accepted as required-native evidence.

103. Final: Ruling: do not broaden the coverage filename regex to exclude inline `internal_integration` modules in production files — the existing report is file-based, inline test modules predate this branch, and excluding their owner would hide production coverage — cost if wrong: inline test lines may remain in coverage totals; no pure-production percentage or complete test-line exclusion is claimed.

104. Final: Ruling: leave the disclosed schema/ONNX Runtime, model, native CPU fmetrics, optional AVM/VVenC, SVT capability, screenshot-warning and flake gaps unchanged — these are unavailable acceptance paths or pre-existing limitations, not demonstrated regressions from this naming/boundary refactor; users receive the explicit limits above, not invented passing claims — cost if wrong: platform/native defects or flakes may still surface when those gates become available; run the affected acceptance gates before relying on them.
