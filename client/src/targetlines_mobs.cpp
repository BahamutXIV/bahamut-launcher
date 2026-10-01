#include "targetlines_mobs.h"

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <span>
#include <string_view>

namespace bahamut_client
{
namespace
{

constexpr std::uint16_t kActorInstantiateOpcode      = 0x00CCu;
constexpr std::size_t   kActorInstantiatePayloadSize = 0x108u;
constexpr std::size_t   kClassNameOffset             = 0x24u;
constexpr std::size_t   kClassNameWidth              = 0x20u;
constexpr std::size_t   kInitParamsOffset            = 0x44u;
constexpr std::uint32_t kNpcActorKind                = 4u;
constexpr std::uint32_t kActorKindShift              = 28u;

constexpr std::uint8_t kLuaParamInt32  = 0x00u;
constexpr std::uint8_t kLuaParamString = 0x02u;
constexpr std::uint8_t kLuaParamTrue   = 0x03u;
constexpr std::uint8_t kLuaParamFalse  = 0x04u;
constexpr std::uint8_t kLuaParamEnd    = 0x0Fu;

struct MobClass
{
    std::uint32_t    actorClassId;
    std::string_view clientClassPath;
};

// Exact paths for actorClassIds used by current Bahamut ambient templates.
// Pool/profile ownership is bahamut:data/mob_pools.yaml and
// bahamut:data/zones/*/mobs.yaml; path identity is bahamut:sql/actorclass.sql.
// The reviewed opening Jellyfish exception is bahamut:sql/spawn_actor_replays.sql:55
// and bahamut:sql/actor_spawn_locations.sql:536.
constexpr MobClass kAmbientMobClasses[] = {
    { 2100101u, "/chara/npc/monster/winglizard/WinglizardLesserStandard" },
    { 2100104u, "/chara/npc/monster/winglizard/WinglizardStandard" },
    { 2100105u, "/chara/npc/monster/winglizard/WinglizardStandard" },
    { 2100107u, "/chara/npc/monster/winglizard/WinglizardStandard" },
    { 2100109u, "/chara/npc/monster/winglizard/WinglizardStandard" },
    { 2100111u, "/chara/npc/monster/winglizard/WinglizardStandard" },
    { 2100113u, "/chara/npc/monster/winglizard/WinglizardStandard" },
    { 2100114u, "/chara/npc/monster/winglizard/WinglizardNormalNM" },
    { 2100203u, "/chara/npc/monster/raptor/RaptorStandard" },
    { 2100207u, "/chara/npc/monster/raptor/RaptorStandard" },
    { 2100211u, "/chara/npc/monster/raptor/RaptorStandard" },
    { 2100301u, "/chara/npc/monster/serow/SerowFemaleStandard" },
    { 2100303u, "/chara/npc/monster/serow/SerowFemaleStandard" },
    { 2100305u, "/chara/npc/monster/serow/SerowMaleStandard" },
    { 2100309u, "/chara/npc/monster/serow/SerowFemaleNM" },
    { 2100314u, "/chara/npc/monster/serow/SerowFemaleStandard" },
    { 2100315u, "/chara/npc/monster/serow/SerowMaleStandard" },
    { 2100401u, "/chara/npc/monster/griffin/GriffinStandard" },
    { 2100403u, "/chara/npc/monster/griffin/GriffinStandard" },
    { 2100405u, "/chara/npc/monster/griffin/GriffinLesserStandard" },
    { 2100406u, "/chara/npc/monster/griffin/GriffinLesserStandard" },
    { 2100407u, "/chara/npc/monster/griffin/GriffinStandard" },
    { 2100503u, "/chara/npc/monster/monkey/MonkeyStandard" },
    { 2100505u, "/chara/npc/monster/monkey/MonkeyStandard" },
    { 2100506u, "/chara/npc/monster/monkey/MonkeyStandard" },
    { 2100509u, "/chara/npc/monster/monkey/MonkeyStandard" },
    { 2100510u, "/chara/npc/monster/monkey/MonkeyStandard" },
    { 2100516u, "/chara/npc/monster/monkey/MonkeyLesserStandard" },
    { 2100602u, "/chara/npc/monster/fly/FlyStandard" },
    { 2100604u, "/chara/npc/monster/fly/FlyStandard" },
    { 2100605u, "/chara/npc/monster/fly/FlyStandard" },
    { 2100607u, "/chara/npc/monster/fly/FlyStandard" },
    { 2100609u, "/chara/npc/monster/fly/FlyStandard" },
    { 2100701u, "/chara/npc/monster/basilisk/BasiliskStandard" },
    { 2100702u, "/chara/npc/monster/basilisk/BasiliskStandard" },
    { 2100705u, "/chara/npc/monster/basilisk/BasiliskStandard" },
    { 2100709u, "/chara/npc/monster/basilisk/BasiliskStandard" },
    { 2100713u, "/chara/npc/monster/basilisk/BasiliskStandard" },
    { 2100714u, "/chara/npc/monster/basilisk/BasiliskStandard" },
    { 2100717u, "/chara/npc/monster/basilisk/BasiliskLesserNM" },
    { 2100801u, "/chara/npc/monster/kujata/KujataHornedNM" },
    { 2100901u, "/chara/npc/monster/cactus/CactusLesserStandard" },
    { 2100905u, "/chara/npc/monster/cactus/CactusNormalStandard" },
    { 2100907u, "/chara/npc/monster/cactus/CactusNormalStandard" },
    { 2100910u, "/chara/npc/monster/cactus/CactusNormalNM" },
    { 2101001u, "/chara/npc/monster/morbol/MorbolNormalStandard" },
    { 2101011u, "/chara/npc/monster/morbol/MorbolLesserNMf0f4" },
    { 2101102u, "/chara/npc/monster/spider/SpiderMaleStandard" },
    { 2101104u, "/chara/npc/monster/spider/SpiderMaleStandard" },
    { 2101108u, "/chara/npc/monster/spider/SpiderFemaleStandard" },
    { 2101111u, "/chara/npc/monster/spider/SpiderChildStandard" },
    { 2101117u, "/chara/npc/monster/spider/SpiderChildStandard" },
    { 2101202u, "/chara/npc/monster/bird/BirdStandard" },
    { 2101203u, "/chara/npc/monster/bird/BirdStandard" },
    { 2101211u, "/chara/npc/monster/bird/BirdStandard" },
    { 2101307u, "/chara/npc/monster/salamander/SalamanderStandard" },
    { 2101309u, "/chara/npc/monster/salamander/SalamanderStandard" },
    { 2101311u, "/chara/npc/monster/salamander/SalamanderStandard" },
    { 2101317u, "/chara/npc/monster/salamander/SalamanderStandard" },
    { 2101318u, "/chara/npc/monster/salamander/SalamanderStandard" },
    { 2101319u, "/chara/npc/monster/salamander/SalamanderStandard" },
    { 2101403u, "/chara/npc/monster/wolf/HyaenaStandard" },
    { 2101404u, "/chara/npc/monster/wolf/WolfStandard" },
    { 2101406u, "/chara/npc/monster/wolf/WolfStandard" },
    { 2101409u, "/chara/npc/monster/wolf/WolfStandard" },
    { 2101413u, "/chara/npc/monster/wolf/WolfWolfNM" },
    { 2101416u, "/chara/npc/monster/wolf/WolfStandard" },
    { 2101418u, "/chara/npc/monster/wolf/WolfStandard" },
    { 2101422u, "/chara/npc/monster/wolf/WolfStandard" },
    { 2101429u, "/chara/npc/monster/wolf/WolfStandard" },
    { 2101501u, "/chara/npc/monster/boar/BoarStandard" },
    { 2101502u, "/chara/npc/monster/boar/BoarStandard" },
    { 2101503u, "/chara/npc/monster/boar/BoarStandard" },
    { 2101507u, "/chara/npc/monster/boar/BoarStandard" },
    { 2101508u, "/chara/npc/monster/boar/BoarStandard" },
    { 2101601u, "/chara/npc/monster/bomb/BombStandard" },
    { 2101602u, "/chara/npc/monster/bomb/BombStandard" },
    { 2101603u, "/chara/npc/monster/bomb/BombStandard" },
    { 2101604u, "/chara/npc/monster/bomb/BombStandard" },
    { 2101607u, "/chara/npc/monster/bomb/BombStandard" },
    { 2101608u, "/chara/npc/monster/bomb/BombNormalStandard" },
    { 2101609u, "/chara/npc/monster/bomb/BombStandard" },
    { 2101614u, "/chara/npc/monster/bomb/BombStandard" },
    { 2101701u, "/chara/npc/monster/ahriman/AhrimanStandard" },
    { 2101703u, "/chara/npc/monster/ahriman/AhrimanStandard" },
    { 2101707u, "/chara/npc/monster/ahriman/AhrimanStandard" },
    { 2101709u, "/chara/npc/monster/ahriman/AhrimanNormalNmMob" },
    { 2101710u, "/chara/npc/monster/ahriman/AhrimanNormalNM" },
    { 2101711u, "/chara/npc/monster/ahriman/AhrimanStandard" },
    { 2101801u, "/chara/npc/monster/livingdead/LivingdeadStandard" },
    { 2101803u, "/chara/npc/monster/livingdead/LivingdeadStandard" },
    { 2101805u, "/chara/npc/monster/livingdead/LivingdeadLancerStandard" },
    { 2101807u, "/chara/npc/monster/livingdead/LivingdeadStandard" },
    { 2101901u, "/chara/npc/monster/skeleton/SkeletonStandard" },
    { 2101903u, "/chara/npc/monster/skeleton/SkeletonGladiatorStandard" },
    { 2101904u, "/chara/npc/monster/skeleton/SkeletonGladiatorStandard" },
    { 2101905u, "/chara/npc/monster/skeleton/SkeletonGladiatorStandard" },
    { 2101908u, "/chara/npc/monster/skeleton/SkeletonStandard" },
    { 2102001u, "/chara/npc/monster/dodo/DodoLesserStandard" },
    { 2102003u, "/chara/npc/monster/dodo/DodoStandard" },
    { 2102005u, "/chara/npc/monster/dodo/DodoStandard" },
    { 2102007u, "/chara/npc/monster/dodo/DodoStandard" },
    { 2102009u, "/chara/npc/monster/dodo/DodoStandard" },
    { 2102010u, "/chara/npc/monster/dodo/DodoNormalNM" },
    { 2102101u, "/chara/npc/monster/orebeetle/OrebeetleStandard" },
    { 2102102u, "/chara/npc/monster/orebeetle/OrebeetleStandard" },
    { 2102105u, "/chara/npc/monster/orebeetle/OrebeetleStandard" },
    { 2102108u, "/chara/npc/monster/orebeetle/OrebeetleStandard" },
    { 2102109u, "/chara/npc/monster/orebeetle/OrebeetleStandard" },
    { 2102111u, "/chara/npc/monster/orebeetle/OrebeetleStandard" },
    { 2102127u, "/chara/npc/monster/orebeetle/OrebeetleStandard" },
    { 2102128u, "/chara/npc/monster/orebeetle/OrebeetleStandard" },
    { 2102201u, "/chara/npc/monster/scalelizard/ScalelizardFireStandard" },
    { 2102203u, "/chara/npc/monster/scalelizard/ScalelizardFireStandard" },
    { 2102204u, "/chara/npc/monster/scalelizard/ScalelizardFireStandard" },
    { 2102205u, "/chara/npc/monster/scalelizard/ScalelizardFireStandard" },
    { 2102207u, "/chara/npc/monster/scalelizard/ScalelizardFireStandard" },
    { 2102209u, "/chara/npc/monster/scalelizard/ScalelizardStandard" },
    { 2102220u, "/chara/npc/monster/scalelizard/ScalelizardStandard" },
    { 2102221u, "/chara/npc/monster/scalelizard/ScalelizardFireNM" },
    { 2102226u, "/chara/npc/monster/scalelizard/ScalelizardStandard" },
    { 2102301u, "/chara/npc/monster/yak/YakMaleStandard" },
    { 2102303u, "/chara/npc/monster/yak/YakMaleStandard" },
    { 2102309u, "/chara/npc/monster/yak/YakMaleNmMob" },
    { 2102310u, "/chara/npc/monster/yak/YakFemaleNmMob" },
    { 2102311u, "/chara/npc/monster/yak/YakMaleNM" },
    { 2102312u, "/chara/npc/monster/yak/YakMaleStandard" },
    { 2102313u, "/chara/npc/monster/yak/YakFemaleStandard" },
    { 2102502u, "/chara/npc/monster/ogre/OgreLesserStandard" },
    { 2102503u, "/chara/npc/monster/ogre/OgreLesserStandard" },
    { 2102504u, "/chara/npc/monster/ogre/OgreLesserStandard" },
    { 2102601u, "/chara/npc/monster/imp/ImpLesserStandard" },
    { 2102605u, "/chara/npc/monster/imp/ImpNormalStandard" },
    { 2102701u, "/chara/npc/monster/flower/FlowerPoisonousStandard" },
    { 2102714u, "/chara/npc/monster/flower/FlowerPoisonousStandard" },
    { 2102715u, "/chara/npc/monster/flower/FlowerPoisonousStandard" },
    { 2102717u, "/chara/npc/monster/flower/FlowerPoisonousStandard" },
    { 2102718u, "/chara/npc/monster/flower/FlowerPoisonousNM" },
    { 2102721u, "/chara/npc/monster/flower/FlowerPoisonousStandard" },
    { 2102722u, "/chara/npc/monster/flower/FlowerPoisonousStandard" },
    { 2102805u, "/chara/npc/monster/treant/TreantLesserStandard" },
    { 2102901u, "/chara/npc/monster/apkallu/ApkalluStandard" },
    { 2102902u, "/chara/npc/monster/apkallu/ApkalluStandard" },
    { 2102903u, "/chara/npc/monster/apkallu/ApkalluStandard" },
    { 2102904u, "/chara/npc/monster/apkallu/ApkalluStandard" },
    { 2102906u, "/chara/npc/monster/apkallu/ApkalluStandard" },
    { 2103005u, "/chara/npc/monster/termite/TermiteSoldierNM" },
    { 2103101u, "/chara/npc/monster/gigantoad/GigantoadStandard" },
    { 2103102u, "/chara/npc/monster/gigantoad/GigantoadStandard" },
    { 2103103u, "/chara/npc/monster/gigantoad/GigantoadStandard" },
    { 2103301u, "/chara/npc/monster/goobbue/GoobbueStandard" },
    { 2103302u, "/chara/npc/monster/goobbue/GoobbueStandard" },
    { 2103401u, "/chara/npc/monster/flan/FlanNormalStandard" },
    { 2103904u, "/chara/npc/monster/bug/LadybugStandard" },
    { 2103906u, "/chara/npc/monster/bug/LadybugStandard" },
    { 2103907u, "/chara/npc/monster/bug/LadybugStandard" },
    { 2103910u, "/chara/npc/monster/bug/LadybugStandard" },
    { 2104007u, "/chara/npc/monster/lemming/NuteaterStandard" },
    { 2104009u, "/chara/npc/monster/lemming/HareStandard" },
    { 2104011u, "/chara/npc/monster/lemming/HareStandard" },
    { 2104019u, "/chara/npc/monster/lemming/GlirulusStandard" },
    { 2104021u, "/chara/npc/monster/lemming/HareStandard" },
    { 2104022u, "/chara/npc/monster/lemming/HareStandard" },
    { 2104102u, "/chara/npc/monster/bat/BatNormalStandard" },
    { 2104103u, "/chara/npc/monster/bat/BatStandard" },
    { 2104105u, "/chara/npc/monster/bat/BatStandard" },
    { 2104106u, "/chara/npc/monster/bat/BatStandard" },
    { 2104107u, "/chara/npc/monster/bat/BatStandard" },
    { 2104108u, "/chara/npc/monster/bat/BatStandard" },
    { 2104109u, "/chara/npc/monster/bat/BatStandard" },
    { 2104201u, "/chara/npc/monster/slug/SlugStandard" },
    { 2104205u, "/chara/npc/monster/slug/SlugStandard" },
    { 2104210u, "/chara/npc/monster/slug/SlugStandard" },
    { 2104212u, "/chara/npc/monster/slug/SlugNormalNM" },
    { 2104215u, "/chara/npc/monster/slug/SlugStandard" },
    { 2104302u, "/chara/npc/monster/petitghost/PetitghostStandard" },
    { 2104312u, "/chara/npc/monster/petitghost/PetitghostStandard" },
    { 2104401u, "/chara/npc/monster/kesaranpasaran/KesaranpasaranStandard" },
    { 2104402u, "/chara/npc/monster/kesaranpasaran/KesaranpasaranStandard" },
    { 2104403u, "/chara/npc/monster/kesaranpasaran/KesaranpasaranStandard" },
    { 2104501u, "/chara/npc/monster/piranha/PiranhaSandyStandard" },
    { 2104504u, "/chara/npc/monster/piranha/PiranhaSandyStandard" },
    { 2104505u, "/chara/npc/monster/piranha/PiranhaBoggyStandard" },
    { 2104506u, "/chara/npc/monster/piranha/PiranhaBoggyStandard" },
    { 2104508u, "/chara/npc/monster/piranha/PiranhaSandyStandard" },
    { 2104511u, "/chara/npc/monster/piranha/PiranhaSandyStandard" },
    { 2104901u, "/chara/npc/monster/elemental/EarthelementalStandard" },
    { 2105001u, "/chara/npc/monster/elemental/LightningelementalStandard" },
    { 2105101u, "/chara/npc/monster/elemental/WarterelementalStandard" },
    { 2105301u, "/chara/npc/monster/crowd/CrowdBeeStandard" },
    { 2105307u, "/chara/npc/monster/crowd/CrowdFlyStandard" },
    { 2105313u, "/chara/npc/monster/crowd/CrowdBeeStandard" },
    { 2105401u, "/chara/npc/monster/jellyfish/JellyfishNormalStandard" },
    { 2105403u, "/chara/npc/monster/jellyfish/JellyfishNormalStandard" },
    { 2105404u, "/chara/npc/monster/jellyfish/JellyfishNormalStandard" },
    { 2105405u, "/chara/npc/monster/jellyfish/JellyfishLesserStandard" },
    { 2105408u, "/chara/npc/monster/jellyfish/JellyfishLesserStandard" },
    { 2105503u, "/chara/npc/monster/longlegs/LonglegsStandard" },
    { 2105505u, "/chara/npc/monster/longlegs/LonglegsStandard" },
    { 2105507u, "/chara/npc/monster/longlegs/LonglegsStandard" },
    { 2105513u, "/chara/npc/monster/longlegs/HarvestmanNM" },
    { 2105601u, "/chara/npc/monster/chigoe/ChigoeStandard" },
    { 2105605u, "/chara/npc/monster/chigoe/ChigoeStandard" },
    { 2105709u, "/chara/npc/monster/mole/MoleEchidnaStandard" },
    { 2105717u, "/chara/npc/monster/mole/MoleMoleStandard" },
    { 2105721u, "/chara/npc/monster/mole/MoleMoleStandard" },
    { 2105722u, "/chara/npc/monster/mole/MoleMoleStandard" },
    { 2105723u, "/chara/npc/monster/mole/MoleMoleStandard" },
    { 2105801u, "/chara/npc/monster/firefly/FireflyNormalStandard" },
    { 2105804u, "/chara/npc/monster/firefly/FireflyNormalStandard" },
    { 2105901u, "/chara/npc/monster/funguar/FunguarLesserStandard" },
    { 2105902u, "/chara/npc/monster/funguar/FunguarStandard" },
    { 2105909u, "/chara/npc/monster/funguar/FunguarStandard" },
    { 2105915u, "/chara/npc/monster/funguar/FunguarNormalNM" },
    { 2105916u, "/chara/npc/monster/funguar/FunguarStandard" },
    { 2106001u, "/chara/npc/monster/sheep/SheepLesserStandard" },
    { 2106002u, "/chara/npc/monster/sheep/SheepLesserStandard" },
    { 2106011u, "/chara/npc/monster/sheep/SheepLesserStandard" },
    { 2106022u, "/chara/npc/monster/sheep/SheepLesserStandard" },
    { 2106201u, "/chara/npc/monster/sprite/SpriteStandard" },
    { 2106213u, "/chara/npc/monster/sprite/SpriteStandard" },
    { 2106214u, "/chara/npc/monster/sprite/SpritePurpleLesserStandard" },
    { 2106221u, "/chara/npc/monster/sprite/SpriteWhiteNormalNMf0f4" },
    { 2106222u, "/chara/npc/monster/sprite/SpriteStandard" },
    { 2106303u, "/chara/npc/monster/qiqirn/QiqirnBarehandsStandard" },
    { 2106304u, "/chara/npc/monster/qiqirn/QiqirnBarehandsStandard" },
    { 2106306u, "/chara/npc/monster/qiqirn/QiqirnBarehandsStandard" },
    { 2106311u, "/chara/npc/monster/qiqirn/QiqirnBarehandsNM" },
    { 2106401u, "/chara/npc/monster/birdman/BirdmanGladiatorStandard" },
    { 2106403u, "/chara/npc/monster/birdman/BirdmanGladiatorStandard" },
    { 2106411u, "/chara/npc/monster/birdman/BirdmanGladiatorStandard" },
    { 2106412u, "/chara/npc/monster/birdman/BirdmanGladiatorStandard" },
    { 2106413u, "/chara/npc/monster/birdman/BirdmanGladiatorStandard" },
    { 2106417u, "/chara/npc/monster/birdman/BirdmanConjurerStandard" },
    { 2106420u, "/chara/npc/monster/birdman/BirdmanConjurerStandard" },
    { 2106421u, "/chara/npc/monster/birdman/BirdmanConjurerStandard" },
    { 2106426u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106432u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106435u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106436u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106437u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106441u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106444u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106445u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106446u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106447u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106449u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106451u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106453u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106455u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106458u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106459u, "/chara/npc/monster/birdman/BirdmanExcPublicFortGuard01" },
    { 2106460u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106463u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106465u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106466u, "/chara/npc/monster/birdman/BirdmanGladiatorPublicFortNM01" },
    { 2106467u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106470u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106472u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2106537u, "/chara/npc/monster/lizardman/LizardmanStandard" },
    { 2106542u, "/chara/npc/monster/lizardman/LizardmanPugilistStandard" },
    { 2106601u, "/chara/npc/monster/gnole/GnoleConjurerStandard" },
    { 2106602u, "/chara/npc/monster/gnole/GnoleConjurerStandard" },
    { 2106604u, "/chara/npc/monster/gnole/GnoleConjurerStandard" },
    { 2106613u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2106622u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2106625u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2106634u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2106643u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2106645u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2106650u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2106651u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2106655u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2106656u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2106701u, "/chara/npc/monster/pixie/PixieStandard" },
    { 2106707u, "/chara/npc/monster/pixie/PixieStandard" },
    { 2106713u, "/chara/npc/monster/pixie/PixieStandard" },
    { 2107003u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2107004u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2107601u, "/chara/npc/monster/crab/CrabStandard" },
    { 2107602u, "/chara/npc/monster/crab/CrabStandard" },
    { 2107604u, "/chara/npc/monster/crab/CrabStandard" },
    { 2107606u, "/chara/npc/monster/crab/CrabLesserStandard" },
    { 2107607u, "/chara/npc/monster/crab/CrabStandard" },
    { 2107611u, "/chara/npc/monster/crab/CrabStandard" },
    { 2107613u, "/chara/npc/monster/crab/CrabStandard" },
    { 2107614u, "/chara/npc/monster/crab/CrabNormalNM" },
    { 2107617u, "/chara/npc/monster/crab/CrabStandard" },
    { 2109001u, "/chara/npc/monster/killermachine/KillermachineLesserStandard" },
    { 2109002u, "/chara/npc/monster/killermachine/KillermachineNormalStandard" },
    { 2109004u, "/chara/npc/monster/killermachine/KillermachineNormalStandard" },
    { 2109005u, "/chara/npc/monster/killermachine/KillermachineNormalStandard" },
    { 2109901u, "/chara/npc/monster/willothewisp/WillOTheWispNormalStandard" },
    { 2109902u, "/chara/npc/monster/willothewisp/WillOTheWispNormalStandard" },
    { 2109903u, "/chara/npc/monster/willothewisp/WillOTheWispNormalStandard" },
    { 2109906u, "/chara/npc/monster/willothewisp/WillOTheWispNormalStandard" },
    { 2109907u, "/chara/npc/monster/willothewisp/WillOTheWispNormalStandard" },
    { 2109908u, "/chara/npc/monster/willothewisp/WillOTheWispNormalStandard" },
    { 2109912u, "/chara/npc/monster/willothewisp/WillOTheWispNormalStandard" },
    { 2109913u, "/chara/npc/monster/willothewisp/WillOTheWispNormalStandard" },
    { 2109914u, "/chara/npc/monster/willothewisp/WillOTheWispNormalStandard" },
    { 2109915u, "/chara/npc/monster/willothewisp/WillOTheWispNormalStandard" },
    { 2110301u, "/chara/npc/monster/goblin/GoblinBommerGlaStandard" },
    { 2110302u, "/chara/npc/monster/goblin/GoblinBommerGlaStandard" },
    { 2110303u, "/chara/npc/monster/goblin/GoblinBommerGlaStandard" },
    { 2110304u, "/chara/npc/monster/goblin/GoblinBommerGlaStandard" },
    { 2110305u, "/chara/npc/monster/goblin/GoblinBommerGlaStandard" },
    { 2110306u, "/chara/npc/monster/goblin/GoblinBommerGlaStandard" },
    { 2110311u, "/chara/npc/monster/goblin/GoblinBommerGlaStandard" },
    { 2110312u, "/chara/npc/monster/goblin/GoblinBommerGlaNM" },
    { 2162004u, "/chara/npc/monster/lizardman/LizardmanArcherStandard" },
    { 2162013u, "/chara/npc/monster/lizardman/LizardmanLancerStandard" },
    { 2162022u, "/chara/npc/monster/lizardman/LizardmanPugilistStandard" },
    { 2162028u, "/chara/npc/monster/lizardman/LizardmanThaumaturgeStandard" },
    { 2162034u, "/chara/npc/monster/lizardman/LizardmanArcherStandard" },
    { 2162039u, "/chara/npc/monster/lizardman/LizardmanLancerStandard" },
    { 2162045u, "/chara/npc/monster/lizardman/LizardmanLncPublicFortGuard01" },
    { 2162046u, "/chara/npc/monster/lizardman/LizardmanPugilistStandard" },
    { 2162051u, "/chara/npc/monster/lizardman/LizardmanPglPublicFortNM01" },
    { 2162052u, "/chara/npc/monster/lizardman/LizardmanThaumaturgeStandard" },
    { 2162053u, "/chara/npc/monster/lizardman/LizardmanThaumaturgeStandard" },
    { 2162054u, "/chara/npc/monster/lizardman/LizardmanThaumaturgeStandard" },
    { 2162056u, "/chara/npc/monster/lizardman/LizardmanThaumaturgeStandard" },
    { 2162058u, "/chara/npc/monster/lizardman/LizardmanThaumaturgeStandard" },
    { 2162061u, "/chara/npc/monster/lizardman/LizardmanThmPublicIfritNM01" },
    { 2180009u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180010u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180011u, "/chara/npc/monster/empire/EmpireGlaScenarioMainLv046" },
    { 2180012u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180013u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180014u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180015u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180016u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180017u, "/chara/npc/monster/empire/EmpireArcScenarioMainLv046" },
    { 2180018u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180019u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180020u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180021u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180022u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180037u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2180102u, "/chara/npc/monster/fighter/FighterEnemyPugilistStandard" },
    { 2180116u, "/chara/npc/monster/fighter/FighterEnemyPugilistStandard" },
    { 2180122u, "/chara/npc/monster/fighter/FighterEnemyPugilistStandard" },
    { 2180125u, "/chara/npc/monster/fighter/FighterEnemyLancerStandard" },
    { 2180202u, "/chara/npc/monster/fighter/FighterEnemyGladiatorStandard" },
    { 2180214u, "/chara/npc/monster/fighter/FighterEnemyGladiatorStandard" },
    { 2180215u, "/chara/npc/monster/fighter/FighterEnemyArcherStandard" },
    { 2180217u, "/chara/npc/monster/fighter/FighterEnemyMarauderStandard" },
    { 2200401u, "/chara/npc/monster/griffin/GriffinStandard" },
    { 2200602u, "/chara/npc/monster/fly/FlyStandard" },
    { 2200604u, "/chara/npc/monster/fly/FlyStandard" },
    { 2201101u, "/chara/npc/monster/spider/SpiderMaleStandard" },
    { 2201103u, "/chara/npc/monster/spider/SpiderChildStandard" },
    { 2201405u, "/chara/npc/monster/wolf/WolfStandard" },
    { 2202008u, "/chara/npc/monster/dodo/DodoStandard" },
    { 2202205u, "/chara/npc/monster/scalelizard/ScalelizardFireStandard" },
    { 2202301u, "/chara/npc/monster/yak/YakMaleStandard" },
    { 2202304u, "/chara/npc/monster/yak/YakFemaleStandard" },
    { 2203001u, "/chara/npc/monster/termite/TermiteStandard" },
    { 2203101u, "/chara/npc/monster/gigantoad/GigantoadStandard" },
    { 2204021u, "/chara/npc/monster/lemming/LemmingStandard" },
    { 2204202u, "/chara/npc/monster/slug/SlugStandard" },
    { 2204501u, "/chara/npc/monster/piranha/PiranhaSandyStandard" },
    { 2205401u, "/chara/npc/monster/jellyfish/JellyfishStandard" },
    { 2205402u, "/chara/npc/monster/jellyfish/JellyfishStandard" },
    { 2205403u, "/chara/npc/monster/jellyfish/JellyfishScenarioLimsaLv00" },
    { 2205404u, "/chara/npc/monster/jellyfish/JellyfishStandard" },
    { 2205411u, "/chara/npc/monster/jellyfish/JellyfishStandard" },
    { 2206409u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2206410u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2206411u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2206423u, "/chara/npc/monster/birdman/BirdmanGladiatorStandard" },
    { 2206429u, "/chara/npc/monster/birdman/BirdmanGladiatorStandard" },
    { 2206430u, "/chara/npc/monster/birdman/BirdmanLancerStandard" },
    { 2206431u, "/chara/npc/monster/birdman/BirdmanConjurerStandard" },
    { 2206432u, "/chara/npc/monster/birdman/BirdmanStandard" },
    { 2206525u, "/chara/npc/monster/lizardman/LizardmanStandard" },
    { 2206526u, "/chara/npc/monster/lizardman/LizardmanStandard" },
    { 2206527u, "/chara/npc/monster/lizardman/LizardmanThaumaturgeStandard" },
    { 2206606u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2206607u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2206608u, "/chara/npc/monster/gnole/GnoleStandard" },
    { 2207601u, "/chara/npc/monster/crab/CrabStandard" },
    { 2207604u, "/chara/npc/monster/crab/CrabStandard" },
    { 2209903u, "/chara/npc/monster/willothewisp/WillOTheWispNormalStandard" },
    { 2280053u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2280056u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2280057u, "/chara/npc/monster/empire/EmpireStandard" },
    { 2280114u, "/chara/npc/monster/fighter/FighterEnemyPugilistStandard" },
    { 2280120u, "/chara/npc/monster/fighter/FighterEnemyPugilistStandard" },
};

class LuaReader
{
public:
    LuaReader(std::span<const std::uint8_t> bytes, std::size_t offset)
    : bytes_(bytes)
    , offset_(offset)
    {
    }

    [[nodiscard]] bool ReadString(std::string_view& value)
    {
        if (!ReadType(kLuaParamString))
        {
            return false;
        }

        const std::size_t start = offset_;
        while (offset_ < bytes_.size() && bytes_[offset_] != 0u)
        {
            ++offset_;
        }
        if (offset_ == bytes_.size())
        {
            return false;
        }

        value = std::string_view(
            reinterpret_cast<const char*>(bytes_.data() + start),
            offset_ - start);
        ++offset_;
        return !value.empty();
    }

    [[nodiscard]] bool ReadBoolean(bool expected)
    {
        return ReadType(expected ? kLuaParamTrue : kLuaParamFalse);
    }

    [[nodiscard]] bool ReadInt32(std::uint32_t expected)
    {
        std::uint32_t actual = 0u;
        return ReadInt32Value(actual) && actual == expected;
    }

    [[nodiscard]] bool ReadInt32Value(std::uint32_t& value)
    {
        if (!ReadType(kLuaParamInt32) || bytes_.size() - offset_ < 4u)
        {
            return false;
        }

        value =
            (static_cast<std::uint32_t>(bytes_[offset_]) << 24u) |
            (static_cast<std::uint32_t>(bytes_[offset_ + 1u]) << 16u) |
            (static_cast<std::uint32_t>(bytes_[offset_ + 2u]) << 8u) |
            static_cast<std::uint32_t>(bytes_[offset_ + 3u]);
        offset_ += 4u;
        return true;
    }

    [[nodiscard]] bool ReadType(std::uint8_t expected)
    {
        if (offset_ >= bytes_.size() || bytes_[offset_] != expected)
        {
            return false;
        }

        ++offset_;
        return true;
    }

    [[nodiscard]] bool ReadEndAndPadding()
    {
        if (!ReadType(kLuaParamEnd))
        {
            return false;
        }

        return std::all_of(
            bytes_.begin() + static_cast<std::ptrdiff_t>(offset_),
            bytes_.end(),
            [](std::uint8_t value)
            {
                return value == 0u;
            });
    }

private:
    std::span<const std::uint8_t> bytes_;
    std::size_t                   offset_;
};

[[nodiscard]] bool ValidActor(std::uint32_t actorId)
{
    // Bahamut's MakeNpcActorId uses kind nibble 4; player IDs must not enter
    // the mob set even if a malformed packet carries a mob profile.
    return actorId != 0u && actorId != 0xFFFFFFFFu && actorId != 0xC0000000u &&
           (actorId >> kActorKindShift) == kNpcActorKind;
}

[[nodiscard]] std::string_view ReadFixedText(
    std::span<const std::uint8_t> bytes,
    std::size_t                   offset,
    std::size_t                   width)
{
    std::size_t length = 0;
    while (length < width && bytes[offset + length] != 0u)
    {
        ++length;
    }

    if (length < width && std::any_of(
                              bytes.begin() + static_cast<std::ptrdiff_t>(offset + length + 1u),
                              bytes.begin() + static_cast<std::ptrdiff_t>(offset + width),
                              [](std::uint8_t value)
                              {
                                  return value != 0u;
                              }))
    {
        return {};
    }

    return std::string_view(
        reinterpret_cast<const char*>(bytes.data() + offset),
        length);
}

[[nodiscard]] std::string_view ClassNameFromPath(std::string_view classPath)
{
    const std::size_t separator = classPath.find_last_of('/');
    if (separator == std::string_view::npos || separator + 1u >= classPath.size())
    {
        return {};
    }

    return classPath.substr(separator + 1u);
}

[[nodiscard]] bool IsExplicitAllyClassPath(std::string_view classPath)
{
    return classPath == "/chara/npc/monster/fighter/FighterAllyOpeningAttacker" ||
           classPath == "/chara/npc/monster/fighter/FighterAllyOpeningHealer";
}

[[nodiscard]] bool IsReviewedMobClass(
    std::uint32_t    actorClassId,
    std::string_view classPath)
{
    if (IsExplicitAllyClassPath(classPath))
    {
        return false;
    }

    for (const MobClass& candidate : kAmbientMobClasses)
    {
        if (candidate.actorClassId == actorClassId &&
            candidate.clientClassPath == classPath)
        {
            return true;
        }
    }

    return false;
}

[[nodiscard]] bool ReadOpeningMobProfile(LuaReader& reader)
{
    if (!reader.ReadBoolean(true) || !reader.ReadBoolean(true) ||
        !reader.ReadInt32(10u) || !reader.ReadInt32(0u) ||
        !reader.ReadInt32(1u) || !reader.ReadBoolean(false))
    {
        return false;
    }

    for (int index = 0; index < 7; ++index)
    {
        if (!reader.ReadBoolean(false))
        {
            return false;
        }
    }

    return reader.ReadInt32(0u) && reader.ReadEndAndPadding();
}

} // namespace

bool IsTargetlinesMobInstantiation(const packet_observer::GameMessage& message)
{
    if (message.direction != packet_observer::Direction::Incoming ||
        message.opcode != kActorInstantiateOpcode ||
        !ValidActor(message.sourceId) ||
        message.payload.size() != kActorInstantiatePayloadSize)
    {
        return false;
    }

    const std::span<const std::uint8_t> payload(message.payload);
    const std::string_view              className =
        ReadFixedText(payload, kClassNameOffset, kClassNameWidth);

    LuaReader        reader(payload, kInitParamsOffset);
    std::string_view classPath;
    if (className.empty() || !reader.ReadString(classPath))
    {
        return false;
    }

    for (int index = 0; index < 5; ++index)
    {
        if (!reader.ReadBoolean(false))
        {
            return false;
        }
    }

    std::uint32_t actorClassId = 0u;
    if (!reader.ReadInt32Value(actorClassId))
    {
        return false;
    }

    // The typed Lua serializer writes actorClassId as a big-endian int32.
    if (ClassNameFromPath(classPath) != className ||
        !IsReviewedMobClass(actorClassId, classPath))
    {
        return false;
    }

    return ReadOpeningMobProfile(reader);
}

} // namespace bahamut_client
