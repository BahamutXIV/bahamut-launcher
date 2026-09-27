#pragma once

#include <cstdint>
#include <string_view>

namespace bahamut_client
{

// English display names from XIVLegacy/xivl-client-data:csv/_zoneParam.csv
// SHA-256 1e75e434217e8d99848ac1d690a9fcd93e43c9a2b00fc983e3ba7fb592d377f9
// joined to csv/xtx_placeName.csv column 1 (English), SHA-256
// 81467ef42e8aeba82fe95f6c4249356550c02e41e6734abf9fdd194051dc1714.
// Place-name ID 1501 is a placeholder. Its zones have no display label.
[[nodiscard]] constexpr std::string_view AreaNameForZone(std::uint32_t zoneId)
{
    switch (zoneId)
    {
        case 128u:
            return "Lower La Noscea";
        case 129u:
            return "Western La Noscea";
        case 130u:
            return "Eastern La Noscea";
        case 131u:
            return "Mistbeard Cove";
        case 132u:
            return "Cassiopeia Hollow";
        case 133u:
            return "Limsa Lominsa";
        case 134u:
            return "Market Wards";
        case 135u:
            return "Upper La Noscea";
        case 137u:
            return "U'Ghamaro Mines";
        case 138u:
            return "La Noscea";
        case 139u:
            return "The Cieldalaes";
        case 140u:
            return "Sailors Ward";
        case 141u:
            return "Lower La Noscea";
        case 143u:
            return "Coerthas Central Highlands";
        case 144u:
            return "Coerthas Eastern Highlands";
        case 145u:
            return "Coerthas Eastern Lowlands";
        case 146u:
            return "Coerthas";
        case 147u:
            return "Coerthas Central Lowlands";
        case 148u:
            return "Coerthas Western Highlands";
        case 150u:
            return "Central Shroud";
        case 151u:
            return "East Shroud";
        case 152u:
            return "North Shroud";
        case 153u:
            return "West Shroud";
        case 154u:
            return "South Shroud";
        case 155u:
            return "Gridania";
        case 156u:
            return "The Black Shroud";
        case 157u:
            return "The Mun-Tuy Cellars";
        case 158u:
            return "The Tam-Tara Deepcroft";
        case 159u:
            return "The Thousand Maws of Toto-Rak";
        case 160u:
            return "Market Wards";
        case 161u:
            return "Peasants Ward";
        case 162u:
            return "Central Shroud";
        case 164u:
            return "Central Shroud";
        case 165u:
            return "Central Shroud";
        case 166u:
            return "Central Shroud";
        case 167u:
            return "Central Shroud";
        case 168u:
            return "Central Shroud";
        case 170u:
            return "Central Thanalan";
        case 171u:
            return "Eastern Thanalan";
        case 172u:
            return "Western Thanalan";
        case 173u:
            return "Northern Thanalan";
        case 174u:
            return "Southern Thanalan";
        case 175u:
            return "Ul'dah";
        case 176u:
            return "Nanawa Mines";
        case 178u:
            return "Copperbell Mines";
        case 179u:
            return "Thanalan";
        case 180u:
            return "Market Wards";
        case 181u:
            return "Merchants Ward";
        case 182u:
            return "Central Thanalan";
        case 184u:
            return "Ul'dah";
        case 185u:
            return "Ul'dah";
        case 186u:
            return "Ul'dah";
        case 187u:
            return "Ul'dah";
        case 188u:
            return "Ul'dah";
        case 190u:
            return "Mor Dhona";
        case 192u:
            return "Rhotano Sea";
        case 193u:
            return "Rhotano Sea";
        case 194u:
            return "Rhotano Sea";
        case 195u:
            return "Rhotano Sea";
        case 196u:
            return "Rhotano Sea";
        case 198u:
            return "Rhotano Sea";
        case 200u:
            return "Strait of Merlthor";
        case 204u:
            return "Western La Noscea";
        case 205u:
            return "Eastern La Noscea";
        case 206u:
            return "Gridania";
        case 207u:
            return "North Shroud";
        case 208u:
            return "South Shroud";
        case 209u:
            return "Ul'dah";
        case 210u:
            return "Eastern Thanalan";
        case 211u:
            return "Western Thanalan";
        case 230u:
            return "Limsa Lominsa";
        case 231u:
            return "Dzemael Darkhold";
        case 232u:
            return "Maelstrom Command";
        case 233u:
            return "Hall of Flames";
        case 234u:
            return "Adders' Nest";
        case 235u:
            return "Shposhae";
        case 236u:
            return "Locke's Lie";
        case 237u:
            return "Turtleback Island";
        case 238u:
            return "Thornmarch";
        case 239u:
            return "The Howling Eye";
        case 240u:
            return "The Bowl of Embers";
        case 244u:
            return "Inn Room";
        case 245u:
            return "The Aurum Vale";
        case 246u:
            return "Cutter's Cry";
        case 247u:
            return "North Shroud";
        case 248u:
            return "Western La Noscea";
        case 249u:
            return "Eastern Thanalan";
        case 250u:
            return "The Howling Eye";
        case 251u:
            return "Transmission Tower";
        case 252u:
            return "The Aurum Vale";
        case 253u:
            return "The Aurum Vale";
        case 254u:
            return "Cutter's Cry";
        case 255u:
            return "Cutter's Cry";
        case 256u:
            return "The Howling Eye";
        case 257u:
            return "Rivenroad";
        case 258u:
            return "North Shroud";
        case 259u:
            return "North Shroud";
        case 260u:
            return "Western La Noscea";
        case 261u:
            return "Western La Noscea";
        case 262u:
            return "Eastern Thanalan";
        case 263u:
            return "Eastern Thanalan";
        case 264u:
            return "Transmission Tower";
        case 265u:
            return "The Bowl of Embers";
        case 266u:
            return "Mor Dhona";
        case 267u:
            return "Rivenroad";
        case 268u:
            return "Rivenroad";
        case 269u:
            return "Locke's Lie";
        case 270u:
            return "Turtleback Island";
        default:
            return {};
    }
}

// Zone-to-region membership follows Bahamut sql/zones.sql (id, region_id).
// Service and battle regions use the general Eorzea heading.
[[nodiscard]] constexpr std::string_view AreaRegionNameForZone(std::uint32_t zoneId)
{
    switch (zoneId)
    {
        case 128u:
        case 129u:
        case 130u:
        case 131u:
        case 132u:
        case 133u:
        case 135u:
        case 137u:
        case 138u:
        case 140u:
        case 141u:
        case 204u:
        case 205u:
        case 230u:
        case 235u:
        case 236u:
        case 237u:
        case 248u:
        case 260u:
        case 261u:
        case 269u:
        case 270u:
            return "La Noscea";
        case 143u:
        case 144u:
        case 145u:
        case 146u:
        case 147u:
        case 148u:
        case 231u:
        case 239u:
        case 245u:
        case 250u:
        case 252u:
        case 253u:
        case 256u:
            return "Coerthas";
        case 150u:
        case 151u:
        case 152u:
        case 153u:
        case 154u:
        case 155u:
        case 156u:
        case 157u:
        case 158u:
        case 159u:
        case 161u:
        case 162u:
        case 206u:
        case 207u:
        case 208u:
        case 238u:
        case 247u:
        case 258u:
        case 259u:
            return "The Black Shroud";
        case 170u:
        case 171u:
        case 172u:
        case 173u:
        case 174u:
        case 175u:
        case 176u:
        case 178u:
        case 179u:
        case 182u:
        case 186u:
        case 187u:
        case 188u:
        case 209u:
        case 210u:
        case 211u:
        case 240u:
        case 246u:
        case 249u:
        case 254u:
        case 255u:
        case 262u:
        case 263u:
        case 265u:
            return "Thanalan";
        case 190u:
        case 251u:
        case 264u:
        case 266u:
            return "Mor Dhona";
        default:
            return AreaNameForZone(zoneId).empty() ? std::string_view{}
                                                   : std::string_view{ "Eorzea" };
    }
}

} // namespace bahamut_client
