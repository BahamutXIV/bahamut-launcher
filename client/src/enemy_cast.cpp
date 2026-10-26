#include "enemy_cast.h"

#include "actor_name.h"
#include "packet_observer.h"
#include "player_state.h"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <iterator>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace
{

// XIVLegacy/xivl-opcodes:opcodes.json, MapClientbound 0x0139-0x013C;
// data/battle_result_semantics.json rows: X01/X10/X18/X00 capacities 1/10/18/0.
// 0x013B has a static shape only; these records do not establish cast duration.
constexpr std::uint16_t kBattleActionSingleTarget = 0x0139u;
constexpr std::uint16_t kBattleActionMultiTarget  = 0x013Au;
constexpr std::uint16_t kBattleActionLarge        = 0x013Bu;
constexpr std::uint16_t kBattleActionNoEffects    = 0x013Cu;
constexpr std::uint16_t kAddActor                 = 0x00CAu;
constexpr std::uint16_t kRemoveActor              = 0x00CBu;
constexpr std::uint16_t kDeleteAllActors          = 0x0007u;
// XIVLegacy/xivl-opcodes:data/achievement_lifecycle.json#s2c-0134-actor-state:
// Map clientbound 0x0134 carries the main-state byte at application offset 0.
constexpr std::uint16_t kSetActorState            = 0x0134u;
constexpr std::uint8_t  kActorPassive             = 0u;
constexpr std::uint8_t  kActorDead                = 1u;
constexpr std::uint8_t  kActorActive              = 2u;
constexpr std::uint8_t  kActorDead2               = 3u;
constexpr std::size_t   kMaximumActiveCasts       = 256u;
constexpr auto          kMinimumVisible           = std::chrono::milliseconds(1800);
constexpr auto          kCompletedHold            = std::chrono::seconds(5);
constexpr auto          kEngagedHold              = std::chrono::seconds(15);
constexpr std::uint32_t kCastAnimationMask        = 0xFF000000u;
constexpr std::uint32_t kCastAnimationPrefix      = 0x6F000000u;
constexpr std::uint32_t kInterruptedAnimation     = 0x7F000002u;
constexpr std::uint16_t kTextIdCasting            = 30128u;
constexpr std::uint16_t kTextIdReadying           = 30126u;
constexpr std::uint16_t kTextIdInterrupted        = 30201u;
constexpr std::size_t   kSourceActorIdOffset      = 0x00u;
constexpr std::size_t   kAnimationIdOffset        = 0x04u;
constexpr std::size_t   kActionCountOffset        = 0x20u;
constexpr std::size_t   kCommandIdOffset          = 0x24u;
constexpr std::size_t   kMinimumBattleActionBytes = 0x28u;
constexpr std::size_t   kActionTargetIdsOffset    = 0x28u;
constexpr std::size_t   kSingleTargetCapacity     = 1u;
constexpr std::size_t   kMultiTargetCapacity      = 10u;
constexpr std::size_t   kLargeTargetCapacity      = 18u;
constexpr std::size_t   kSingleTargetTextIdOffset = 0x2Eu;
constexpr std::size_t   kMultiTargetTextIdOffset  = 0x64u;
constexpr std::size_t   kLargeActionTextIdOffset  = 0x94u;

struct CommandInfo
{
    std::uint16_t    commandId;
    std::string_view name;
    std::uint32_t    castTimeMs;
};

// Derived from bahamut/sql/battle_commands.sql at cd462f080d56579d1266ae2ac034fe1319e4645f
// (SHA-256 259EA64373604371E2C2E76EF00043DDFB1D3F5AEDA5025E4C578EF404904D26).
// CREATE TABLE maps id, name, and cast_time to columns 1, 2, and 32; placeholder names are omitted.
constexpr CommandInfo kCommandCatalog[] = {
    { 22302u, "phantom_dart", 2000u },
    { 22303u, "spirit_dart", 2000u },
    { 23001u, "attack", 3000u },
    { 23002u, "attack", 3000u },
    { 23003u, "attack", 3000u },
    { 23004u, "attack", 3000u },
    { 23005u, "attack", 3000u },
    { 23006u, "attack", 3000u },
    { 23007u, "attack", 3000u },
    { 23008u, "attack", 3000u },
    { 23009u, "nutkick", 3000u },
    { 23010u, "somersault_slash", 3000u },
    { 23011u, "bark_to_arms", 3000u },
    { 23012u, "nutshell", 3000u },
    { 23013u, "regurgitate", 3000u },
    { 23014u, "rancid_belch", 3000u },
    { 23015u, "rancid_belch", 3000u },
    { 23016u, "rancid_belch", 3000u },
    { 23017u, "fowl_stench", 3000u },
    { 23018u, "fowl_stench", 3000u },
    { 23019u, "fowl_stench", 3000u },
    { 23020u, "regurgitate", 3000u },
    { 23021u, "vorpal_roar", 3000u },
    { 23022u, "cold_gaze", 3000u },
    { 23023u, "stone_gaze", 3000u },
    { 23024u, "sand_breath", 3000u },
    { 23025u, "heavy_stomp", 3000u },
    { 23026u, "body_slam", 3000u },
    { 23027u, "wall_of_scales", 3000u },
    { 23028u, "foul_breath", 3000u },
    { 23029u, "ripper_fang", 3000u },
    { 23030u, "foul_breath", 3000u },
    { 23031u, "scythe_tail", 3000u },
    { 23032u, "fireball", 3000u },
    { 23033u, "self-destruct", 3000u },
    { 23034u, "combustion", 3000u },
    { 23036u, "fast_burn", 3000u },
    { 23037u, "tendril_spines", 3000u },
    { 23038u, "binding_tendrils", 3000u },
    { 23039u, "numbing_tendrils", 3000u },
    { 23040u, "firespit", 3000u },
    { 23041u, "double_smash", 3000u },
    { 23042u, "elbow_drop", 3000u },
    { 23043u, "inferno_drop", 3000u },
    { 23044u, "bone_breaker", 3000u },
    { 23045u, "booming_bellow", 3000u },
    { 23046u, "primal_scream", 3000u },
    { 23047u, "guano_lob", 3000u },
    { 23048u, "ultrasonics", 3000u },
    { 23049u, "blood_drain", 3000u },
    { 23050u, "digestive_ooze", 3000u },
    { 23051u, "mortal_mist", 3000u },
    { 23052u, "gravel_burst", 3000u },
    { 23055u, "breakout", 3000u },
    { 23056u, "spinout", 3000u },
    { 23057u, "cliff_cry", 3000u },
    { 23058u, "backflip", 3000u },
    { 23059u, "tail_chase", 3000u },
    { 23060u, "brine_blast", 3000u },
    { 23061u, "rage_of_the_deep", 3000u },
    { 23062u, "feeding_frenzy", 3000u },
    { 23063u, "shockbreath", 3000u },
    { 23064u, "brundleflight", 3000u },
    { 23066u, "thunderstrike", 3000u },
    { 23067u, "thunderwall", 3000u },
    { 23068u, "thunderstorm", 3000u },
    { 23069u, "prickle", 3000u },
    { 23070u, "hair_ball", 3000u },
    { 23071u, "furfall", 3000u },
    { 23072u, "fursquall", 3000u },
    { 23073u, "reaching_scratch", 3000u },
    { 23074u, "drop_kick", 3000u },
    { 23075u, "deep_scratch", 3000u },
    { 23076u, "soil_smear", 3000u },
    { 23077u, "sneeze", 3000u },
    { 23078u, "stampede", 3000u },
    { 23079u, "hoofkick", 3000u },
    { 23080u, "gale_switch", 3000u },
    { 23081u, "shriek", 3000u },
    { 23082u, "overtone_shriek", 3000u },
    { 23083u, "beak_snap", 3000u },
    { 23084u, "bombardier", 3000u },
    { 23085u, "spoil", 3000u },
    { 23086u, "seedvolley", 3000u },
    { 23087u, "germinate", 3000u },
    { 23088u, "seedspray", 3000u },
    { 23089u, "sough", 3000u },
    { 23090u, "bleat", 3000u },
    { 23091u, "head_butt", 3000u },
    { 23092u, "alpine_fury", 3000u },
    { 23093u, "seismic_scream", 3000u },
    { 23094u, "level_5_petrify", 3000u },
    { 23095u, "aural_vacuum", 3000u },
    { 23096u, "tail_whip", 3000u },
    { 23097u, "caterwaul", 3000u },
    { 23098u, "aqueous_discharge", 3000u },
    { 23099u, "viscous_discharge", 3000u },
    { 23100u, "mucous_discharge", 3000u },
    { 23101u, "tail_strike", 3000u },
    { 23102u, "frenzied_claw", 3000u },
    { 23103u, "forward_kick", 3000u },
    { 23104u, "squawk", 3000u },
    { 23105u, "basilisk_breath", 3000u },
    { 23106u, "stomp", 3000u },
    { 23113u, "mine_dust", 3000u },
    { 23114u, "cluster_geyser", 3000u },
    { 23115u, "sphere_of_cold", 3000u },
    { 23116u, "impish_incantations", 3000u },
    { 23117u, "impish_incantations", 3000u },
    { 23118u, "impish_incantations", 3000u },
    { 23119u, "stardust", 3000u },
    { 23121u, "quickchant", 3000u },
    { 23122u, "frenetic_flurry", 3000u },
    { 23123u, "romp", 3000u },
    { 23124u, "triple_tumble", 3000u },
    { 23125u, "forbidden_magicks", 3000u },
    { 23127u, "dark_cloud", 3000u },
    { 23128u, "curse", 3000u },
    { 23129u, "grave_reel", 3000u },
    { 23130u, "gate_to_oblivion", 3000u },
    { 23132u, "brackish_bubble", 3000u },
    { 23133u, "claw_guard", 3000u },
    { 23134u, "bubble_shower", 3000u },
    { 23135u, "backclip", 3000u },
    { 23138u, "piercing_din", 3000u },
    { 23139u, "corrosive_spit", 3000u },
    { 23140u, "brain_spike", 3000u },
    { 23141u, "vertical_lash", 3000u },
    { 23142u, "midnight_howl", 3000u },
    { 23143u, "threatening_growl", 3000u },
    { 23144u, "foul_bite", 1000u },
    { 23145u, "sanguine_bite", 3000u },
    { 23146u, "needleshot", 3000u },
    { 23147u, "1000_needles", 3000u },
    { 23148u, "tender_thrust", 3000u },
    { 23149u, "sun_spines", 3000u },
    { 23150u, "desert_respite", 3000u },
    { 23151u, "aetherial_wave", 3000u },
    { 23152u, "aetherial_barrier", 3000u },
    { 23153u, "aetherial_torrent", 3000u },
    { 23154u, "snort", 3000u },
    { 23155u, "reckless_charge", 3000u },
    { 23156u, "bristle", 3000u },
    { 23157u, "bellowing_grunt", 3000u },
    { 23158u, "circlespark", 3000u },
    { 23159u, "thundervapor", 3000u },
    { 23160u, "kinetic_pulse", 3000u },
    { 23161u, "kinetic_pulse", 3000u },
    { 23162u, "kinetic_pulse", 3000u },
    { 23163u, "magnetic_pulse", 3000u },
    { 23164u, "magnetic_pulse", 3000u },
    { 23165u, "magnetic_pulse", 3000u },
    { 23166u, "stampede", 3000u },
    { 23167u, "head_butt", 3000u },
    { 23168u, "quickchant", 3000u },
    { 23169u, "quickchant", 3000u },
    { 23170u, "quickchant", 3000u },
    { 23171u, "quickchant", 3000u },
    { 23172u, "quickchant", 3000u },
    { 23173u, "forbidden_magicks", 3000u },
    { 23174u, "forbidden_magicks", 3000u },
    { 23175u, "forbidden_magicks", 3000u },
    { 23176u, "forbidden_magicks", 3000u },
    { 23177u, "forbidden_magicks", 3000u },
    { 23178u, "aetherial_wave", 3000u },
    { 23179u, "aetherial_wave", 3000u },
    { 23180u, "aetherial_wave", 3000u },
    { 23181u, "aetherial_wave", 3000u },
    { 23182u, "aetherial_wave", 3000u },
    { 23183u, "titan''s_flesh", 3000u },
    { 23184u, "titan''s_soul", 3000u },
    { 23185u, "titan''s_anger", 3000u },
    { 23187u, "titan''s_boon", 3000u },
    { 23188u, "ranged_attack", 3000u },
    { 23189u, "lay_of_the_leaves", 3000u },
    { 23190u, "woodland_lullaby", 3000u },
    { 23191u, "ranged_attack", 3000u },
    { 23192u, "wind_claw", 3000u },
    { 23193u, "updraft", 3000u },
    { 23194u, "sunset_plumes", 3000u },
    { 23195u, "swift_gust", 3000u },
    { 23196u, "swift_gale", 3000u },
    { 23197u, "ranged_attack", 3000u },
    { 23198u, "devastate", 3000u },
    { 23199u, "decimate", 3000u },
    { 23200u, "pulverize", 3000u },
    { 23201u, "scrutinize", 3000u },
    { 23202u, "infuriate", 3000u },
    { 23203u, "mud_sling", 3000u },
    { 23204u, "kibosh", 3000u },
    { 23205u, "sandspray", 3000u },
    { 23206u, "misdirection", 3000u },
    { 23207u, "flying_sardine", 3000u },
    { 23208u, "yawn", 3000u },
    { 23209u, "flying_marlin", 3000u },
    { 23210u, "loam_cough", 3000u },
    { 23211u, "violent_sneeze", 3000u },
    { 23212u, "violent_sneeze", 3000u },
    { 23213u, "beatdown", 3000u },
    { 23214u, "violent_sneeze", 3000u },
    { 23215u, "inhale", 3000u },
    { 23216u, "ranged_attack", 3000u },
    { 23217u, "deft_slash", 3000u },
    { 23223u, "imperial_guard", 3000u },
    { 23224u, "magitek_cannon", 3000u },
    { 23225u, "ranged_attack", 3000u },
    { 23226u, "trap_jaws", 3000u },
    { 23227u, "stridulation", 3000u },
    { 23228u, "mandible_bite", 3000u },
    { 23229u, "formic_pheromones", 3000u },
    { 23230u, "stone''s_throw", 3000u },
    { 23231u, "rot_breath", 3000u },
    { 23232u, "cloud_of_pestilence", 3000u },
    { 23233u, "feather_flurry", 3000u },
    { 23234u, "wing_cutter", 3000u },
    { 23235u, "yolk_shower", 3000u },
    { 23236u, "deafening_caw", 3000u },
    { 23237u, "silencing_caw", 3000u },
    { 23238u, "wooly_spindle", 3000u },
    { 23239u, "lullaby", 3000u },
    { 23240u, "dream_bubble", 3000u },
    { 23244u, "doomwave", 3000u },
    { 23245u, "magicked_skull", 3000u },
    { 23246u, "shadow_sickle", 3000u },
    { 23247u, "minions_of_the_pit", 3000u },
    { 23250u, "slumber_spores", 3000u },
    { 23251u, "slumber_cloud", 3000u },
    { 23254u, "queasy_spores", 3000u },
    { 23255u, "hypha_whip", 3000u },
    { 23256u, "queasy_cloud", 3000u },
    { 23257u, "agitation", 3000u },
    { 23258u, "stellar_flame", 3000u },
    { 23259u, "blinding_burst", 3000u },
    { 23260u, "innernova", 3000u },
    { 23261u, "hypernova", 3000u },
    { 23262u, "sticky_tongue", 3000u },
    { 23263u, "ranine_stare", 3000u },
    { 23264u, "slimy_secretion", 3000u },
    { 23265u, "livid_lap", 3000u },
    { 23266u, "gut_press", 3000u },
    { 23269u, "ranine_glare", 3000u },
    { 23270u, "caudal_spine", 3000u },
    { 23271u, "smoulder", 3000u },
    { 23272u, "surge", 3000u },
    { 23273u, "raging_horn", 3000u },
    { 23274u, "burning_cyclone", 3000u },
    { 23275u, "levinshower", 3000u },
    { 23276u, "flames_of_defiance", 3000u },
    { 23277u, "bolt_of_defiance", 3000u },
    { 23278u, "serpentine_tail", 3000u },
    { 23280u, "ranged_attack", 3000u },
    { 23281u, "sour_breath", 3000u },
    { 23282u, "vine_probe", 3000u },
    { 23283u, "sweeping_lash", 3000u },
    { 23284u, "tendril_tremor", 3000u },
    { 23285u, "bad_breath", 3000u },
    { 23286u, "sweet_breath", 3000u },
    { 23287u, "ranged_attack", 3000u },
    { 23288u, "caustic_blow", 3000u },
    { 23289u, "deadly_thrust", 3000u },
    { 23290u, "realm_shaker", 3000u },
    { 23291u, "deadly_thrust", 3000u },
    { 23294u, "sticky_web", 3000u },
    { 23295u, "ranged_attack", 3000u },
    { 23296u, "pulverizing_pound", 3000u },
    { 23297u, "onrush", 3000u },
    { 23298u, "seismic_rift", 3000u },
    { 23299u, "sonorous_blast", 3000u },
    { 23300u, "terrene_blast", 3000u },
    { 23301u, "remembrance", 3000u },
    { 23302u, "chthonic_call", 3000u },
    { 23305u, "ranged_attack", 3000u },
    { 23306u, "glossolalia", 3000u },
    { 23307u, "amorphic_spear", 3000u },
    { 23308u, "custard_cuff", 3000u },
    { 23310u, "ranged_attack", 3000u },
    { 23311u, "bomb_toss", 2000u },
    { 23312u, "bomb_toss", 5000u },
    { 23313u, "hail_mary", 3000u },
    { 23314u, "hail_mary", 3000u },
    { 23315u, "firecracker_shower", 2000u },
    { 23317u, "ranged_attack", 3000u },
    { 23318u, "charged_whisker", 3000u },
    { 23319u, "charged_whisker", 3000u },
    { 23320u, "charged_whisker", 3000u },
    { 23321u, "blaster", 3000u },
    { 23322u, "chaotic_eye", 3000u },
    { 23323u, "whisker_charge", 3000u },
    { 23324u, "whisker_charge", 3000u },
    { 23328u, "attack", 3000u },
    { 23329u, "attack", 3000u },
    { 23330u, "attack", 3000u },
    { 23331u, "attack", 3000u },
    { 23332u, "attack", 3000u },
    { 23333u, "attack", 3000u },
    { 23334u, "attack", 3000u },
    { 23335u, "attack", 3000u },
    { 23336u, "attack", 3000u },
    { 23337u, "attack", 3000u },
    { 23338u, "attack", 3000u },
    { 23339u, "attack", 3000u },
    { 23340u, "gillbrations", 3000u },
    { 23341u, "good_gillbrations", 3000u },
    { 23342u, "mode_change", 3000u },
    { 23343u, "poison_breath", 3000u },
    { 23344u, "circle_attack", 3000u },
    { 23345u, "stunner", 3000u },
    { 23346u, "soul_eater", 3000u },
    { 23347u, "mucous_discharge", 3000u },
    { 23348u, "corrosive_spit", 3000u },
    { 23349u, "ranged_attack", 3000u },
    { 23350u, "grim_cleaver", 3000u },
    { 23351u, "grim_halo", 3000u },
    { 23352u, "grim_fate", 3000u },
    { 23353u, "corrupted_tail", 3000u },
    { 23354u, "desolation", 3000u },
    { 23355u, "shadow_burst", 3000u },
    { 23356u, "ancient_wrath", 3000u },
    { 23357u, "immortal_effigy", 3000u },
    { 23360u, "attack", 3000u },
    { 23361u, "sear", 3000u },
    { 23362u, "vulcan_burst", 3000u },
    { 23363u, "incinerate", 3000u },
    { 23364u, "eruption", 3000u },
    { 23365u, "crimson_cyclone", 3000u },
    { 23367u, "radiant_plume", 3000u },
    { 23368u, "hellfire", 3000u },
    { 23374u, "eruption", 3000u },
    { 23375u, "eruption", 3000u },
    { 23376u, "radiant_plume", 3000u },
    { 23379u, "death_march", 3000u },
    { 23380u, "death_throes", 3000u },
    { 23384u, "attack", 3000u },
    { 23385u, "heavy_heel", 3000u },
    { 23386u, "plaincracker", 3000u },
    { 23387u, "boulder_clap", 3000u },
    { 23388u, "landslide", 3000u },
    { 23389u, "stone_skull", 3000u },
    { 23390u, "molten_core", 3000u },
    { 23391u, "scree_scatter", 3000u },
    { 23392u, "scree_scatter", 3000u },
    { 23393u, "true_grit", 3000u },
    { 23394u, "wing_clipper", 3000u },
    { 23395u, "strong_shoulder", 3000u },
    { 23396u, "firedamp", 3000u },
    { 23397u, "titan''s_heart", 3000u },
    { 23398u, "titan''s_boon", 3000u },
    { 23401u, "attrition", 3000u },
    { 23402u, "mindwipe", 3000u },
    { 23403u, "sound_of_the_sea", 3000u },
    { 23404u, "radiant_plume", 3000u },
    { 23405u, "attack", 3000u },
    { 23406u, "attack", 3000u },
    { 23407u, "attack", 3000u },
    { 23408u, "hellfire", 3000u },
    { 23409u, "hellfire", 3000u },
    { 23410u, "whisker_charge", 3000u },
    { 23411u, "whisker_charge", 3000u },
    { 23412u, "whisker_charge", 3000u },
    { 23413u, "100_needles", 3000u },
    { 23414u, "mogdive", 1000u },
    { 23415u, "whisker_bash", 1000u },
    { 23417u, "moogle-go-round", 1000u },
    { 23420u, "moogle_eye_shot", 2000u },
    { 23421u, "pom_flare", 4000u },
    { 23422u, "maximoogle", 2000u },
    { 23423u, "mognesia", 4000u },
    { 23424u, "memento_moogle", 3000u },
    { 23438u, "break", 2000u },
    { 23439u, "bough_down", 3000u },
    { 23440u, "switch_swipe", 3000u },
    { 23441u, "switch_swipe", 3000u },
    { 23442u, "canopy", 3000u },
    { 23443u, "arboreal_storm", 3000u },
    { 23444u, "rise_and_fall", 3000u },
    { 23445u, "backhand_blow", 3000u },
    { 23446u, "entangle", 3000u },
    { 23447u, "photosynthesis", 3000u },
    { 23450u, "pinecone_bomb", 3000u },
    { 23451u, "memento_moogle", 3000u },
    { 23452u, "memento_moogle", 3000u },
    { 23453u, "memento_moogle", 3000u },
    { 23456u, "breath_of_the_lion", 3500u },
    { 23457u, "voice_of_the_lion", 3500u },
    { 23458u, "breath_of_the_dragon", 3500u },
    { 23459u, "voice_of_the_dragon", 3500u },
    { 23460u, "breath_of_the_ram", 3500u },
    { 23461u, "voice_of_the_ram", 3500u },
    { 23462u, "dissent_of_the_bat", 3500u },
    { 23463u, "chaotic_chorus", 3500u },
    { 23464u, "the_scorpions_sting", 3500u },
    { 23476u, "10-tonze_swipe", 3000u },
    { 23477u, "10-tonze_swing", 3000u },
    { 23478u, "eye_of_the_beholder", 3000u },
    { 23479u, "glower", 3000u },
    { 23480u, "100-tonze_swipe", 3000u },
    { 23481u, "100-tonze_swing", 3000u },
    { 23482u, "100-tonze_swing", 3000u },
    { 23483u, "giant_swing_recovery", 3000u },
    { 23484u, "animal_instinct", 3000u },
    { 23485u, "steel_cyclone", 3000u },
    { 23486u, "dragon_kick", 3000u },
    { 23487u, "second_wind", 3000u },
    { 23488u, "fists_of_wind", 3000u },
    { 23489u, "whirlwind", 3000u },
    { 23490u, "godsbane", 3000u },
    { 23491u, "bloodbath", 3000u },
    { 23492u, "invigorate", 3000u },
    { 23493u, "jump", 3000u },
    { 23494u, "wyvern_dive", 3000u },
    { 23496u, "ring_of_thorns", 3000u },
    { 23500u, "sandtrap", 3000u },
    { 23501u, "sand_pillar", 3000u },
    { 23502u, "sandtrap", 3000u },
    { 23503u, "sand_pillar", 3000u },
    { 23504u, "sandtrap", 3000u },
    { 23505u, "sand_pillar", 3000u },
    { 23506u, "sand_pillar", 3000u },
    { 23507u, "sand_spire", 3000u },
    { 23508u, "fire_ii", 3000u },
    { 23509u, "burn_ii", 3000u },
    { 23510u, "flare", 3000u },
    { 23511u, "blizzard_ii", 3000u },
    { 23512u, "frost_ii", 3000u },
    { 23513u, "freeze", 3000u },
    { 23514u, "aero_ii", 3000u },
    { 23515u, "choke_ii", 3000u },
    { 23516u, "tornado", 3000u },
    { 23517u, "stone_ii", 3000u },
    { 23518u, "rasp_ii", 3000u },
    { 23519u, "quake", 3000u },
    { 23520u, "thunder_ii", 3000u },
    { 23521u, "shock_ii", 3000u },
    { 23522u, "burst", 3000u },
    { 23523u, "water_ii", 3000u },
    { 23524u, "drown_ii", 3000u },
    { 23525u, "flood", 3000u },
    { 23526u, "blind", 3000u },
    { 23527u, "sleep", 3000u },
    { 23528u, "silence", 3000u },
    { 23529u, "gravity", 3000u },
    { 23530u, "paralyze", 3000u },
    { 23531u, "slow", 3000u },
    { 23532u, "stygian_spikes_ii", 3000u },
    { 23533u, "stoneskin_ii", 3000u },
    { 23534u, "shock_spikes_ii", 3000u },
    { 23535u, "ranged_attack", 3000u },
    { 23536u, "attack", 3000u },
    { 23537u, "downburst", 3000u },
    { 23538u, "wicked_wheel", 3000u },
    { 23539u, "slipstream", 3000u },
    { 23540u, "mistral_song", 3000u },
    { 23541u, "mistral_shriek", 3000u },
    { 23544u, "plumage", 3000u },
    { 23546u, "aerial_blast", 3000u },
    { 23550u, "featherlance", 3000u },
    { 23552u, "thermal_tumult", 3000u },
    { 23556u, "great_whirlwind", 3000u },
    { 23559u, "eye_of_the_storm", 3000u },
    { 23560u, "formic_pheromones", 3000u },
    { 23565u, "cermet_drill", 3000u },
    { 23566u, "drill_cannon", 3000u },
    { 23567u, "overcharge", 3000u },
    { 23568u, "mistral_song", 3000u },
    { 23569u, "mistral_shriek", 3000u },
    { 23570u, "aerial_blast", 3000u },
    { 23571u, "aerial_blast", 3000u },
    { 23572u, "aerial_blast", 3000u },
    { 23573u, "featherlance", 3000u },
    { 23576u, "attack", 3000u },
    { 23577u, "sear", 3000u },
    { 23578u, "vulcan_burst", 3000u },
    { 23579u, "incinerate", 3000u },
    { 23580u, "crimson_cyclone", 3000u },
    { 23581u, "hellfire", 3000u },
    { 23582u, "eruption", 3000u },
    { 23583u, "radiant_plume", 3000u },
    { 23584u, "dia_ii", 3000u },
    { 23586u, "sunset_plumes", 3000u },
    { 23587u, "infuriate", 3000u },
    { 23588u, "grim_fate", 3000u },
    { 23589u, "ranged_attack", 3000u },
    { 23590u, "desolation", 3000u },
    { 23591u, "scrutinize", 3000u },
    { 23592u, "eruption", 3000u },
    { 23593u, "radiant_plume", 3000u },
    { 23594u, "eruption", 3000u },
    { 23595u, "radiant_plume", 3000u },
    { 23596u, "megaflare", 3000u },
    { 23603u, "thermionic_beam", 3000u },
    { 23604u, "thermionic_burst", 3000u },
    { 23605u, "plasma_acoustics", 3000u },
    { 23608u, "dalamud_dive", 3000u },
    { 23609u, "dalamud_dive", 3000u },
    { 23610u, "dalamud_dive", 3000u },
    { 23611u, "iron_chariot", 3000u },
    { 23612u, "lunar_dynamo", 3000u },
    { 23613u, "ravensbeak", 3000u },
    { 23614u, "chaos_thrust", 3000u },
    { 23615u, "twisting_vice", 3000u },
    { 23616u, "explosion", 3000u },
    { 23617u, "aetherial_emission", 3000u },
    { 23618u, "cavernous_maw", 3000u },
    { 23619u, "voidbrume", 3000u },
    { 23620u, "aetherial_eruption", 3000u },
    { 23621u, "aetherial_eruption", 3000u },
    { 23622u, "aetherial_eruption", 3000u },
    { 23623u, "cataclysm", 3000u },
    { 23624u, "megaflare", 3000u },
    { 23625u, "megaflare", 3000u },
    { 23626u, "megaflare", 3000u },
    { 23627u, "lunar_dynamo", 3000u },
    { 23628u, "self-destruct", 3000u },
    { 23629u, "100-tonze_swipe", 3000u },
    { 23630u, "100-tonze_swing", 3000u },
    { 23631u, "fierce_ravensbeak", 3000u },
    { 23638u, "heavy_heel", 3000u },
    { 23639u, "plaincracker", 3000u },
    { 23640u, "boulder_clap", 3000u },
    { 23641u, "landslide", 3000u },
    { 23642u, "switch_swipe", 3000u },
    { 23643u, "switch_swipe", 3000u },
    { 23644u, "explosion", 3000u },
    { 23645u, "explosion", 3000u },
    { 23980u, "ifrit_sear", 3000u },
    { 23981u, "ifrit_vulcan_burst", 3000u },
    { 23982u, "ifrit_incinerate", 3000u },
    { 23983u, "ifrit_eruption", 3000u },
    { 23984u, "ifrit_crimson_cyclone", 3500u },
    { 23985u, "ifrit_radiant_plume", 3000u },
    { 23986u, "ifrit_hellfire", 3000u },
    { 23987u, "ifrit_radiant_plume_center", 3000u },
    { 23988u, "ifrit_radiant_plume_outer", 3000u },
    { 23989u, "garuda_wicked_wheel", 1000u },
    { 23990u, "garuda_slipstream", 3000u },
    { 23991u, "garuda_downburst", 1000u },
    { 23994u, "garuda_mistral_song", 1000u },
    { 23996u, "garuda_aerial_blast", 3000u },
    { 27149u, "holy_succor", 2000u },
    { 27237u, "ballad_of_magi", 3000u },
    { 27238u, "paeon_of_war", 3000u },
    { 27239u, "minuet_of_rigor", 3000u },
    { 27306u, "sleep", 3000u },
    { 27308u, "blizzard", 3000u },
    { 27310u, "fire", 3000u },
    { 27311u, "fira", 5000u },
    { 27312u, "firaga", 8000u },
    { 27313u, "thunder", 2000u },
    { 27315u, "thundaga", 5000u },
    { 27316u, "burst", 4000u },
    { 27317u, "sleepga", 4000u },
    { 27318u, "flare", 8000u },
    { 27319u, "freeze", 5000u },
    { 27346u, "cure", 2000u },
    { 27347u, "cura", 2000u },
    { 27348u, "curaga", 3000u },
    { 27349u, "raise", 10000u },
    { 27350u, "stoneskin", 3000u },
    { 27351u, "protect", 3000u },
    { 27352u, "repose", 3000u },
    { 27353u, "aero", 3000u },
    { 27354u, "aerora", 4000u },
    { 27355u, "stone", 2000u },
    { 27356u, "stonera", 3000u },
    { 27357u, "esuna", 2000u },
    { 27358u, "regen", 2000u },
    { 28562u, "flashfreeze", 3000u },
    { 28563u, "flashfreeze_ii", 3000u },
    { 28565u, "damnation", 3000u },
    { 28566u, "damnation_ii", 3000u },
    { 28588u, "shadowfall", 3000u },
    { 28589u, "silhouette", 3000u },
    { 28597u, "banish", 2000u },
    { 28598u, "banish_ii", 2000u },
    { 28599u, "banish_iii", 2000u },
    { 28600u, "banish_iv", 2000u },
    { 28602u, "bio", 2000u },
    { 28603u, "bio_ii", 2000u },
    { 28604u, "bio_iii", 2000u },
    { 28606u, "dia", 2000u },
    { 28608u, "dia_iii", 2000u },
    { 28618u, "poison", 2000u },
    { 28619u, "poison_ii", 2000u },
    { 28620u, "poison_iii", 2000u },
    { 28634u, "bind", 3000u },
    { 28666u, "sacrifice", 2000u },
    { 28667u, "sacrifice_ii", 2000u },
    { 28668u, "sacrifice_iii", 2000u },
    { 28669u, "sacrifice_iv", 2000u },
    { 28675u, "resurrect", 10000u },
    { 28815u, "purge", 2000u },
    { 28816u, "purge_ii", 2000u },
    { 28817u, "purge_iii", 2000u },
    { 28902u, "levinbolt", 2000u },
    { 28903u, "levinbolt_ii", 2000u },
    { 28928u, "natures_fury", 3000u },
    { 28929u, "elemental_shroud", 3000u },
    { 28932u, "fire", 3000u },
    { 28934u, "fire_iii", 3000u },
    { 28935u, "fire_iv", 3000u },
    { 28937u, "blizzard", 3000u },
    { 28939u, "blizzard_iii", 3000u },
    { 28940u, "blizzard_iv", 3000u },
    { 28942u, "aero", 3000u },
    { 28944u, "aero_iii", 3000u },
    { 28945u, "aero_iv", 3000u },
    { 28947u, "stone", 2000u },
    { 28949u, "stone_iii", 2000u },
    { 28950u, "stone_iv", 2000u },
    { 28952u, "thunder", 2000u },
    { 28954u, "thunder_iii", 2000u },
    { 28955u, "thunder_iv", 2000u },
    { 28957u, "water", 3500u },
    { 28959u, "water_iii", 3500u },
    { 28960u, "water_iv", 3500u },
    { 28962u, "burn", 3000u },
    { 28964u, "burn_iii", 3000u },
    { 28966u, "frost", 3000u },
    { 28968u, "frost_iii", 3000u },
    { 28970u, "choke", 3000u },
    { 28972u, "choke_iii", 3000u },
    { 28974u, "rasp", 2000u },
    { 28976u, "rasp_iii", 2000u },
    { 28978u, "shock", 2000u },
    { 28980u, "shock_iii", 2000u },
    { 28982u, "drown", 3500u },
    { 28984u, "drown_iii", 3500u },
    { 28986u, "flare", 8000u },
    { 28987u, "flare_ii", 8000u },
    { 28989u, "freeze", 5000u },
    { 28990u, "freeze_ii", 5000u },
    { 28998u, "burst", 4000u },
    { 28999u, "burst_ii", 4000u },
    { 29004u, "repose", 3000u },
    { 29005u, "repose_ii", 3000u },
    { 29007u, "sleep", 3000u },
    { 29008u, "sleep_ii", 3000u },
    { 29010u, "cure", 2000u },
    { 29011u, "cure_ii", 2000u },
    { 29012u, "cure_iii", 2000u },
    { 29013u, "cure_iv", 2000u },
    { 29015u, "protect", 3000u },
    { 29016u, "protect_ii", 3000u },
    { 29017u, "protect_iii", 3000u },
    { 29019u, "shell", 3000u },
    { 29020u, "shell_ii", 3000u },
    { 29021u, "shell_iii", 3000u },
    { 29023u, "stoneskin", 3000u },
    { 29025u, "stoneskin_iii", 3000u },
    { 29027u, "shock_spikes", 2000u },
    { 29029u, "shock_spikes_iii", 2000u },
    { 29031u, "poisona", 2000u },
    { 29033u, "paralyna", 2000u },
    { 29035u, "silena", 2000u },
    { 29037u, "raise", 10000u },
    { 29038u, "raise_ii", 10000u },
    { 29046u, "curaga", 3000u },
    { 29047u, "curaga_ii", 3000u },
    { 29497u, "parley", 3500u },
    { 29502u, "hasty_hand", 3500u },
    { 29503u, "piece_by_piece", 3500u },
    { 29504u, "clean_slate", 3500u },
    { 29505u, "grand_design", 3500u },
    { 29506u, "by_the_book", 3500u },
    { 29507u, "unerring_hand", 3500u },
    { 29508u, "tricks_of_the_trade", 3500u },
    { 29509u, "makers_muse", 3500u },
    { 29510u, "greater_calling", 3500u },
    { 29511u, "high_return", 3500u },
    { 29512u, "sure_and_steady", 3500u },
    { 29513u, "bold_endeavor", 3500u },
    { 29514u, "fulfillment", 3500u },
    { 29515u, "masterpiece", 3500u },
    { 29516u, "culmination", 3500u },
    { 29517u, "lifes_purpose", 3500u },
    { 29518u, "grandmastery", 3500u },
    { 29519u, "perfection", 3500u },
    { 29520u, "creative_urge", 3500u },
    { 29521u, "brand_of_fire", 3500u },
    { 29522u, "brand_of_ice", 3500u },
    { 29523u, "brand_of_wind", 3500u },
    { 29524u, "brand_of_earth", 3500u },
    { 29525u, "brand_of_lightning", 3500u },
    { 29526u, "brand_of_water", 3500u },
    { 29527u, "elemental_appeal", 3500u },
    { 29528u, "improvise", 3500u },
    { 29529u, "masters_mend", 3500u },
    { 29530u, "waste_not", 3500u },
    { 29531u, "tender_touch", 3500u },
    { 29532u, "comfort_zone", 3500u },
    { 29533u, "ingenuity", 3500u },
    { 29534u, "determination", 3500u },
    { 29535u, "magnum_opus", 3500u },
    { 29536u, "great_strides", 3500u },
    { 29537u, "eye_for_detail", 3500u },
    { 29538u, "flawlessness", 3500u },
    { 29539u, "meticulous_mind", 3500u },
    { 29540u, "make_the_most", 3500u },
    { 29541u, "byregots_blessing", 3500u },
    { 29542u, "inner_quiet", 3500u },
    { 29543u, "good_measure", 3500u },
    { 29544u, "inspiration", 3500u },
    { 29545u, "pride_of_labor", 3500u },
    { 29546u, "innovation", 3500u },
    { 29547u, "sublimation", 3500u },
    { 29548u, "resolution", 3500u },
    { 29549u, "fit_for_kings", 3500u },
    { 29550u, "manipulation", 3500u },
    { 29551u, "heart_and_soul", 3500u },
    { 29552u, "purification", 3500u },
    { 29553u, "aspect_balance", 3500u },
    { 29772u, "trust", 3000u },
    { 29902u, "supplement", 3500u },
    { 29903u, "immersion", 3500u },
    { 29904u, "epiphany", 3500u },
    { 29905u, "close_inspection", 3500u },
    { 29906u, "harmonize", 3500u },
    { 29907u, "less_is_more", 3500u },
    { 29908u, "devotion", 3500u },
    { 29909u, "recollection", 3500u },
    { 29910u, "assemble", 3500u },
    { 29911u, "blinding_speed", 3500u },
    { 29912u, "preemptive_measures", 3500u },
    { 29913u, "ease_the_burden", 3500u },
    { 29914u, "preserve", 3500u },
    { 29915u, "mass_production", 3500u },
    { 29916u, "extrapolation", 3500u },
    { 29917u, "hand_of_the_gods", 3500u },
    { 29918u, "llymains_ward", 3500u },
};

struct BattleAction
{
    std::uint32_t                                   sourceActorId = 0;
    std::uint32_t                                   animationId   = 0;
    std::uint32_t                                   actionCount   = 0;
    std::uint16_t                                   commandId     = 0;
    std::uint16_t                                   textId        = 0;
    std::array<std::uint32_t, kLargeTargetCapacity> targetActorIds{};
    std::size_t                                     targetCount = 0;
};

bool IsValidActorId(std::uint32_t actorId)
{
    return actorId != 0u && actorId != 0xFFFFFFFFu &&
           actorId != 0xC0000000u && actorId != 0xE0000000u;
}

bool IsValidPlayerState(const bahamut_client::PlayerStateSnapshot& player)
{
    return IsValidActorId(player.actorId) && player.zoneId != 0u &&
           player.zoneId != 0xFFFFFFFFu;
}

bool IsEnemyActor(std::uint32_t actorId, std::uint32_t localActorId)
{
    return IsValidActorId(actorId) && actorId != localActorId &&
           (actorId >> 28u) == 4u;
}

bool ReadU16(const std::vector<std::uint8_t>& bytes,
             std::size_t                      offset,
             std::uint16_t&                   value)
{
    if (offset > bytes.size() || bytes.size() - offset < 2u)
    {
        return false;
    }
    value = static_cast<std::uint16_t>(
        static_cast<std::uint16_t>(bytes[offset]) |
        static_cast<std::uint16_t>(static_cast<std::uint16_t>(bytes[offset + 1u]) << 8u));
    return true;
}

bool ReadU32(const std::vector<std::uint8_t>& bytes,
             std::size_t                      offset,
             std::uint32_t&                   value)
{
    if (offset > bytes.size() || bytes.size() - offset < 4u)
    {
        return false;
    }
    value = static_cast<std::uint32_t>(bytes[offset]) |
            (static_cast<std::uint32_t>(bytes[offset + 1u]) << 8u) |
            (static_cast<std::uint32_t>(bytes[offset + 2u]) << 16u) |
            (static_cast<std::uint32_t>(bytes[offset + 3u]) << 24u);
    return true;
}

bool IsBattleActionOpcode(std::uint16_t opcode)
{
    return opcode == kBattleActionSingleTarget ||
           opcode == kBattleActionMultiTarget ||
           opcode == kBattleActionLarge ||
           opcode == kBattleActionNoEffects;
}

bool ReadBattleAction(const packet_observer::GameMessage& message,
                      BattleAction&                       action,
                      bool&                               isCastStart)
{
    isCastStart         = false;
    const auto& payload = message.payload;
    if (!IsBattleActionOpcode(message.opcode) ||
        payload.size() < kMinimumBattleActionBytes ||
        !ReadU32(payload, kSourceActorIdOffset, action.sourceActorId) ||
        !ReadU32(payload, kAnimationIdOffset, action.animationId) ||
        !ReadU32(payload, kActionCountOffset, action.actionCount) ||
        !ReadU16(payload, kCommandIdOffset, action.commandId))
    {
        return false;
    }

    if (message.opcode == kBattleActionNoEffects || action.actionCount == 0u)
    {
        return true;
    }

    std::size_t textIdOffset   = kSingleTargetTextIdOffset;
    std::size_t targetCapacity = kSingleTargetCapacity;
    if (message.opcode == kBattleActionMultiTarget)
    {
        textIdOffset   = kMultiTargetTextIdOffset;
        targetCapacity = kMultiTargetCapacity;
    }
    else if (message.opcode == kBattleActionLarge)
    {
        textIdOffset   = kLargeActionTextIdOffset;
        targetCapacity = kLargeTargetCapacity;
    }
    if (!ReadU16(payload, textIdOffset, action.textId))
    {
        return false;
    }

    const std::size_t targetCount = std::min<std::size_t>(action.actionCount, targetCapacity);
    action.targetCount            = targetCount;
    for (std::size_t index = 0; index < action.targetCount; ++index)
    {
        if (!ReadU32(payload,
                     kActionTargetIdsOffset + index * sizeof(action.targetActorIds[index]),
                     action.targetActorIds[index]))
        {
            return false;
        }
    }

    isCastStart =
        (action.animationId & kCastAnimationMask) == kCastAnimationPrefix &&
        (action.textId == kTextIdCasting || action.textId == kTextIdReadying);
    return true;
}

const CommandInfo* FindCommand(std::uint16_t commandId)
{
    const auto found = std::lower_bound(
        std::begin(kCommandCatalog),
        std::end(kCommandCatalog),
        commandId,
        [](const CommandInfo& command, std::uint16_t value)
        {
            return command.commandId < value;
        });
    return found != std::end(kCommandCatalog) && found->commandId == commandId
               ? &*found
               : nullptr;
}

} // namespace

namespace bahamut_client
{

EnemyCastService::EnemyCastService(PlayerStateService* playerState, ActorNameService* actorNames)
: playerState_(playerState)
, actorNames_(actorNames)
{
}

void EnemyCastService::Observe(const packet_observer::GameMessage& message)
{
    if (message.direction != packet_observer::Direction::Incoming)
    {
        return;
    }

    if (message.opcode == kDeleteAllActors)
    {
        std::lock_guard<std::mutex> lock(mutex_);
        activeCasts_.clear();
        engaged_.clear();
        localActorId_ = 0;
        zoneId_       = 0;
        return;
    }

    if (message.opcode == kAddActor && message.payload.size() != 8u)
    {
        return;
    }
    if (message.opcode == kRemoveActor || message.opcode == kAddActor)
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (IsValidActorId(localActorId_) && message.sourceId == localActorId_)
        {
            activeCasts_.clear();
            engaged_.clear();
            localActorId_ = 0;
            zoneId_       = 0;
        }
        else
        {
            activeCasts_.erase(message.sourceId);
            engaged_.erase(message.sourceId);
        }
        return;
    }

    if (message.opcode == kSetActorState && message.payload.size() >= 8u)
    {
        const std::uint8_t state = message.payload[0];
        if (state != kActorPassive && state != kActorDead &&
            state != kActorActive && state != kActorDead2)
        {
            return;
        }
        std::lock_guard<std::mutex> lock(mutex_);
        const auto                  player = playerState_ != nullptr ? playerState_->Snapshot() : std::nullopt;
        if (!player.has_value() || !IsValidPlayerState(*player))
        {
            return;
        }
        SyncPlayer(player->actorId, player->zoneId);
        if (!IsEnemyActor(message.sourceId, localActorId_))
        {
            return;
        }
        if (state != kActorActive)
        {
            activeCasts_.erase(message.sourceId);
            engaged_.erase(message.sourceId);
            return;
        }
        if (engaged_.size() >= kMaximumActiveCasts && !engaged_.contains(message.sourceId))
        {
            const auto oldest = std::min_element(
                engaged_.begin(), engaged_.end(), [](const auto& left, const auto& right)
                {
                    return left.second < right.second;
                });
            engaged_.erase(oldest);
        }
        engaged_[message.sourceId] = Clock::now();
        return;
    }

    BattleAction action;
    bool         isCastStart = false;
    if (!ReadBattleAction(message, action, isCastStart) ||
        !IsValidActorId(action.sourceActorId))
    {
        return;
    }

    std::lock_guard<std::mutex> lock(mutex_);
    const auto                  player = playerState_ != nullptr ? playerState_->Snapshot() : std::nullopt;
    if (!player.has_value() || !IsValidPlayerState(*player))
    {
        activeCasts_.clear();
        engaged_.clear();
        localActorId_ = 0;
        zoneId_       = 0;
        return;
    }
    SyncPlayer(player->actorId, player->zoneId);
    const auto now = Clock::now();
    PruneExpired(now);

    const auto rememberEngaged = [&](std::uint32_t actorId)
    {
        const auto engaged = engaged_.find(actorId);
        if (engaged == engaged_.end())
        {
            return;
        }
        engaged->second = now;
    };
    rememberEngaged(action.sourceActorId);
    for (std::size_t index = 0; index < action.targetCount; ++index)
    {
        rememberEngaged(action.targetActorIds[index]);
    }

    if (isCastStart)
    {
        activeCasts_.erase(action.sourceActorId);
        if (!IsEnemyActor(action.sourceActorId, localActorId_))
        {
            return;
        }

        const CommandInfo* command = FindCommand(action.commandId);
        if (action.commandId == 0u)
        {
            return;
        }

        if (activeCasts_.size() >= kMaximumActiveCasts)
        {
            const auto oldest = std::min_element(
                activeCasts_.begin(),
                activeCasts_.end(),
                [](const auto& left, const auto& right)
                {
                    return left.second.startedAt < right.second.startedAt;
                });
            activeCasts_.erase(oldest);
        }

        const std::uint32_t castTimeMs = command != nullptr && command->castTimeMs != 0u
                                             ? command->castTimeMs
                                             : 3500u;
        const auto          duration   = std::max(
            std::chrono::milliseconds(static_cast<std::chrono::milliseconds::rep>(castTimeMs)),
            kMinimumVisible);
        activeCasts_.emplace(
            action.sourceActorId,
            ActiveCast{ action.commandId,
                        command != nullptr ? std::string(command->name)
                                           : "Ability " + std::to_string(action.commandId),
                        now,
                        duration,
                        now + duration + kCompletedHold,
                        false });
        return;
    }

    const auto active = activeCasts_.find(action.sourceActorId);
    if (active != activeCasts_.end() &&
        (action.commandId == 0u || active->second.commandId == action.commandId))
    {
        if (action.animationId == kInterruptedAnimation && action.textId == kTextIdInterrupted)
        {
            activeCasts_.erase(active);
            return;
        }
        active->second.completed = true;
        active->second.removeAt  = std::max(now + kCompletedHold,
                                            active->second.startedAt + kMinimumVisible);
    }
}

std::vector<EnemyCastSnapshot> EnemyCastService::Snapshot()
{
    std::lock_guard<std::mutex> lock(mutex_);
    const auto                  player = playerState_ != nullptr ? playerState_->Snapshot() : std::nullopt;
    if (!player.has_value() || !IsValidPlayerState(*player))
    {
        activeCasts_.clear();
        engaged_.clear();
        localActorId_ = 0;
        zoneId_       = 0;
        return {};
    }
    SyncPlayer(player->actorId, player->zoneId);
    const auto now = Clock::now();
    PruneExpired(now);

    std::vector<EnemyCastSnapshot> snapshot;
    snapshot.reserve(activeCasts_.size() + engaged_.size());
    for (const auto& [actorId, cast] : activeCasts_)
    {
        const auto   deadline         = cast.startedAt + cast.duration;
        const bool   completed        = cast.completed || now >= deadline;
        const double durationSeconds  = std::chrono::duration<double>(cast.duration).count();
        const double remainingSeconds = completed ? 0.0 : std::chrono::duration<double>(deadline - now).count();
        const double progress         = completed ? 1.0 : std::chrono::duration<double>(now - cast.startedAt).count() / durationSeconds;
        snapshot.push_back({
            actorId,
            cast.commandId,
            cast.name,
            actorNames_ != nullptr ? actorNames_->NameFor(actorId) : std::string{},
            remainingSeconds,
            durationSeconds,
            progress,
            true,
            completed,
        });
    }
    for (const auto& entry : engaged_)
    {
        const std::uint32_t actorId = entry.first;
        if (activeCasts_.contains(actorId))
        {
            continue;
        }
        snapshot.push_back({
            actorId,
            0u,
            "Watching for next skill",
            actorNames_ != nullptr ? actorNames_->NameFor(actorId) : std::string{},
            0.0,
            0.0,
            1.0,
            false,
            false,
        });
    }
    std::sort(
        snapshot.begin(),
        snapshot.end(),
        [this](const EnemyCastSnapshot& left, const EnemyCastSnapshot& right)
        {
            if (left.casting != right.casting)
            {
                return left.casting;
            }
            if (left.casting && left.completed != right.completed)
            {
                return !left.completed;
            }
            if (left.casting && left.remainingSeconds != right.remainingSeconds)
            {
                return left.remainingSeconds < right.remainingSeconds;
            }
            if (!left.casting && engaged_.at(left.actorId) != engaged_.at(right.actorId))
            {
                return engaged_.at(left.actorId) > engaged_.at(right.actorId);
            }
            return left.actorId < right.actorId;
        });
    return snapshot;
}

void EnemyCastService::SyncPlayer(std::uint32_t actorId, std::uint32_t zoneId)
{
    if (localActorId_ != actorId || zoneId_ != zoneId)
    {
        activeCasts_.clear();
        engaged_.clear();
        localActorId_ = actorId;
        zoneId_       = zoneId;
    }
}

void EnemyCastService::PruneExpired(Clock::time_point now)
{
    for (auto active = activeCasts_.begin(); active != activeCasts_.end();)
    {
        if (now >= active->second.removeAt)
        {
            active = activeCasts_.erase(active);
        }
        else
        {
            ++active;
        }
    }
    for (auto active = engaged_.begin(); active != engaged_.end();)
    {
        if (now - active->second >= kEngagedHold)
        {
            active = engaged_.erase(active);
        }
        else
        {
            ++active;
        }
    }
}

} // namespace bahamut_client
