-- Editable data, never executed:
-- config/addons/gearsets/sets/<server id>-<character id>/sets.lua.
-- See FILE_HEADER for the format.

local MAX_SETS       = 100
local MAX_NAME_BYTES = 64
local MAX_DEPTH      = 4
local SETS_RECORD    = 'sets'

-- Package positions, matching client/src/equipment_state.h. UNVERIFIED
-- against the client: only the main hand, head, and body positions are
-- corroborated; labels follow the Bahamut server's slot enumeration.
local SLOTS          = { 0, 1, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 19, 21, 22 }
local SLOT_KEYS      =
{
    [0] = 'main',
    [1] = 'off',
    [4] = 'throwing',
    [5] = 'pack',
    [6] = 'pouch',
    [8] = 'head',
    [9] = 'undershirt',
    [10] = 'body',
    [11] = 'undergarment',
    [12] = 'legs',
    [13] = 'hands',
    [14] = 'feet',
    [15] = 'waist',
    [16] = 'neck',
    [17] = 'ears',
    [19] = 'wrists',
    [21] = 'right_ring',
    [22] = 'left_ring',
}
local SLOT_BY_KEY    = {}
for slot, key in pairs(SLOT_KEYS) do SLOT_BY_KEY[key] = slot end

-- Abbreviations and names by 1.x class and job ID, from
-- bahamut:data/enums/class_name.yaml. Abbreviations also name the icons.
local CLASSES       =
{
    [2] = { 'PGL', 'Pugilist' },
    [3] = { 'GLD', 'Gladiator' },
    [4] = { 'MRD', 'Marauder' },
    [7] = { 'ARC', 'Archer' },
    [8] = { 'LNC', 'Lancer' },
    [22] = { 'THM', 'Thaumaturge' },
    [23] = { 'CNJ', 'Conjurer' },
    [29] = { 'CRP', 'Carpenter' },
    [30] = { 'BSM', 'Blacksmith' },
    [31] = { 'ARM', 'Armorer' },
    [32] = { 'GSM', 'Goldsmith' },
    [33] = { 'LTW', 'Leatherworker' },
    [34] = { 'WVR', 'Weaver' },
    [35] = { 'ALC', 'Alchemist' },
    [36] = { 'CUL', 'Culinarian' },
    [39] = { 'MIN', 'Miner' },
    [40] = { 'BTN', 'Botanist' },
    [41] = { 'FSH', 'Fisher' },
}
local JOBS          =
{
    [15] = { 'MNK', 'Monk' },
    [16] = { 'PLD', 'Paladin' },
    [17] = { 'WAR', 'Warrior' },
    [18] = { 'BRD', 'Bard' },
    [19] = { 'DRG', 'Dragoon' },
    [26] = { 'BLM', 'Black Mage' },
    [27] = { 'WHM', 'White Mage' },
}
local ABBREVIATIONS = {}
for _, entry in pairs(CLASSES) do ABBREVIATIONS[entry[1]] = true end

for _, entry in pairs(JOBS) do ABBREVIATIONS[entry[1]] = true end

local FILE_HEADER =
{
    '-- Gear Sets for one character. Gear Sets rewrites this file when sets change',
    '-- in game and keeps only this header. Reopen the window after editing.',
    '-- Each set has a name, an optional class or job abbreviation such as CNJ,',
    '-- and any of these slots:',
    '--   main off throwing pack pouch head undershirt body undergarment legs',
    '--   hands feet waist neck ears wrists right_ring left_ring',
    "-- A slot is 'empty', an item name such as 'WEATHERED_CANE', an item number,",
    "-- or { 'WEATHERED_CANE', copy = '<16 hex digits>' } for one exact copy.",
    '-- Without a copy, Equip uses the only copy you carry. Slots left out keep',
    '-- what you are wearing.',
}

local M           = {}
local Manager     = {}
Manager.__index   = Manager

local function whole(value, maximum)
    return type(value) == 'number' and value >= 0 and value <= maximum and value == math.floor(value)
end

local function number_text(value)
    return string.format('%.0f', value)
end

local function is_valid_utf8(text)
    local index  = 1
    local length = #text
    while index <= length do
        local byte  = string.byte(text, index)
        local extra = 0
        local low   = 0x80
        local high  = 0xBF
        if byte >= 0xC2 and byte <= 0xDF then
            extra = 1
        elseif byte >= 0xE0 and byte <= 0xEF then
            extra = 2
            if byte == 0xE0 then low = 0xA0 end

            if byte == 0xED then high = 0x9F end
        elseif byte >= 0xF0 and byte <= 0xF4 then
            extra = 3
            if byte == 0xF0 then low = 0x90 end

            if byte == 0xF4 then high = 0x8F end
        elseif byte >= 0x80 then
            return false
        end

        if index + extra > length then return false end

        for offset = 1, extra do
            local continuation = string.byte(text, index + offset)
            local floor        = offset == 1 and low or 0x80
            local ceiling      = offset == 1 and high or 0xBF
            if continuation < floor or continuation > ceiling then return false end
        end

        index = index + extra + 1
    end

    return true
end

local function is_valid_name(name)
    return type(name) == 'string' and #name >= 1 and #name <= MAX_NAME_BYTES and not string.find(name, '%c') and string.find(name, '%S') ~= nil and is_valid_utf8(name)
end

local function is_valid_uid(uid)
    return type(uid) == 'string' and #uid == 16 and string.find(uid, '^[0-9a-f]+$') ~= nil and uid ~= '0000000000000000'
end

-- Parses `return` followed by nested tables of strings, integers and booleans.
-- Skips line and block comments without executing code.
-- Returns the value, or nil and 'line <n>: <message>'.
function M.parse(text)
    local position = 1
    local line     = 1
    local length   = #text

    local function fail(message)
        error({ parse = 'line ' .. line .. ': ' .. message }, 0)
    end

    local function advance(count)
        local stop = position + count - 1
        for index = position, stop do
            if string.byte(text, index) == 10 then line = line + 1 end
        end

        position = stop + 1
    end

    local function skip()
        while position <= length do
            local byte = string.byte(text, position)
            if byte == 32 or byte == 9 or byte == 13 or byte == 10 then
                advance(1)
            elseif string.sub(text, position, position + 1) == '--' then
                local level = string.match(text, '^%-%-%[(=*)%[', position)
                if level then
                    local close  = ']' .. level .. ']'
                    local finish = string.find(text, close, position, true)
                    if finish == nil then fail('unfinished comment') end

                    advance(finish + #close - position)
                else
                    local newline = string.find(text, '\n', position, true)
                    advance((newline or length + 1) - position)
                end
            else
                return
            end
        end
    end

    local function word()
        return string.match(text, '^[%a_][%w_]*', position)
    end

    local value

    local function string_value(quote)
        advance(1)
        local parts = {}
        while true do
            local byte = string.byte(text, position)
            if byte == nil or byte == 10 or byte == 13 then fail('unfinished string') end

            local char = string.char(byte)
            if char == quote then
                advance(1)
                return table.concat(parts)
            elseif char == '\\' then
                local escaped = string.sub(text, position + 1, position + 1)
                local mapped  = ({ ['\\'] = '\\', ["'"] = "'", ['"'] = '"', n = '\n', t = '\t' })[escaped]
                if mapped == nil then fail('unsupported escape') end

                parts[#parts + 1] = mapped
                advance(2)
            else
                parts[#parts + 1] = char
                advance(1)
            end
        end
    end

    local function table_value(depth)
        if depth > MAX_DEPTH then fail('tables nest too deeply') end

        advance(1)
        local result = {}
        local count  = 0
        while true do
            skip()
            if string.sub(text, position, position) == '}' then
                advance(1)
                return result
            end

            local key
            local name = word()
            if string.sub(text, position, position) == '[' then
                advance(1)
                key = value(depth)
                skip()
                if string.sub(text, position, position) ~= ']' then fail("expected ']'") end

                advance(1)
                skip()
                if string.sub(text, position, position) ~= '=' then fail("expected '='") end

                advance(1)
            elseif name and string.match(text, '^[%a_][%w_]*%s*=[^=]', position) then
                key = name
                advance(#name)
                skip()
                advance(1)
            else
                count = count + 1
                key   = count
            end

            if type(key) ~= 'string' and type(key) ~= 'number' then fail('unsupported key') end

            if result[key] ~= nil then fail('duplicate field ' .. tostring(key)) end

            result[key] = value(depth)
            skip()
            local separator = string.sub(text, position, position)
            if separator == ',' or separator == ';' then
                advance(1)
            elseif separator ~= '}' then
                fail("expected ',' or '}'")
            end
        end
    end

    value            = function(depth)
        skip()
        local char = string.sub(text, position, position)
        if char == '{' then return table_value(depth + 1) end

        if char == "'" or char == '"' then return string_value(char) end

        local digits = string.match(text, '^%-?%d+', position)
        if digits then
            advance(#digits)
            if string.match(text, '^[%w_.]', position) then fail('only whole numbers are supported') end

            return tonumber(digits)
        end

        local name = word()
        if name == 'true' or name == 'false' then
            advance(#name)
            return name == 'true'
        end

        fail('expected a value')
    end

    local ok, result = pcall(function()
        skip()
        if word() ~= 'return' then fail("expected 'return'") end

        advance(6)
        skip()
        if string.sub(text, position, position) ~= '{' then fail("expected '{'") end

        local parsed = value(0)
        skip()
        if position <= length then fail('unexpected text after the sets') end

        return parsed
    end)
    if ok then return result end

    if type(result) == 'table' and result.parse then return nil, result.parse end

    error(result, 0)
end

local function item_label(entry)
    return bahamut.item_name(entry.catalog) or number_text(entry.catalog)
end

-- One slot from the file: 'empty', an item name or number, or a table with
-- the item first and an optional exact copy.
local function read_slot(value, where)
    if value == 'empty' then return { state = 'empty' } end

    local item = value
    local copy = nil
    if type(value) == 'table' then
        for key in pairs(value) do
            if key ~= 1 and key ~= 'copy' then return nil, where .. ': unknown field ' .. tostring(key) end
        end

        item = value[1]
        copy = type(value.copy) == 'string' and string.lower(value.copy) or value.copy
        if copy ~= nil and not is_valid_uid(copy) then return nil, where .. ': copy must be 16 hex digits' end
    end

    local catalog
    if type(item) == 'number' and whole(item, 4294967295) and item > 0 then
        catalog = item
    elseif type(item) == 'string' then
        catalog = bahamut.item_id(item)
    end

    if catalog == nil then return nil, where .. ': unknown item ' .. tostring(item) end

    return { state = 'item', catalog = catalog, uid = copy }
end

function M.read_sets(data)
    if type(data) ~= 'table' then return nil, 'the file must return a table of sets' end

    local sets, count = {}, 0
    for key in pairs(data) do
        if not whole(key, MAX_SETS) or key < 1 then return nil, 'unexpected field ' .. tostring(key) end

        count = count + 1
    end

    -- With a gap, # may stop short and ipairs would silently drop later sets.
    if count ~= #data then return nil, 'sets must be numbered without gaps' end

    if #data > MAX_SETS then return nil, 'more than ' .. MAX_SETS .. ' sets' end

    for index, entry in ipairs(data) do
        local where = 'set ' .. index
        if type(entry) ~= 'table' then return nil, where .. ': not a table' end

        if not is_valid_name(entry.name) then return nil, where .. ': name must be 1-' .. MAX_NAME_BYTES .. ' bytes of text' end

        local class = entry.class
        if class ~= nil and (type(class) ~= 'string' or not ABBREVIATIONS[string.upper(class)]) then return nil, where .. ': unknown class ' .. tostring(class) end

        local set = { name = entry.name, class = class and string.upper(class) or '', slots = {} }
        for key, value in pairs(entry) do
            if key ~= 'name' and key ~= 'class' then
                local slot = SLOT_BY_KEY[key]
                if slot == nil then return nil, where .. ': unknown field ' .. tostring(key) end

                local parsed, problem = read_slot(value, where .. ' ' .. key)
                if parsed == nil then return nil, problem end

                set.slots[slot] = parsed
            end
        end

        sets[index] = set
    end

    if #sets ~= #data then return nil, 'sets must be numbered without gaps' end

    return sets
end

local function quote(text)
    return "'" .. string.gsub(string.gsub(text, '\\', '\\\\'), "'", "\\'") .. "'"
end

local function slot_text(entry)
    if entry.state == 'empty' then return "'empty'" end

    local name = bahamut.item_name(entry.catalog)
    local item = name and quote(name) or number_text(entry.catalog)
    if entry.uid == nil then return item end

    return '{ ' .. item .. ', copy = ' .. quote(entry.uid) .. ' }'
end

function M.write_sets(sets)
    local lines = {}
    for _, text in ipairs(FILE_HEADER) do lines[#lines + 1] = text end

    lines[#lines + 1] = 'return {'
    for _, set in ipairs(sets) do
        lines[#lines + 1] = '    {'
        lines[#lines + 1] = '        name = ' .. quote(set.name) .. ','
        if set.class ~= '' then lines[#lines + 1] = '        class = ' .. quote(set.class) .. ',' end

        for _, slot in ipairs(SLOTS) do
            local entry = set.slots[slot]
            if entry ~= nil then lines[#lines + 1] = '        ' .. SLOT_KEYS[slot] .. ' = ' .. slot_text(entry) .. ',' end
        end

        lines[#lines + 1] = '    },'
    end

    lines[#lines + 1] = '}'
    return table.concat(lines, '\n') .. '\n'
end

local function same_entry(left, right)
    if left == nil or right == nil then return left == right end

    return left.state == right.state and left.catalog == right.catalog and left.uid == right.uid
end

local function same_sets(left, right)
    if #left ~= #right then return false end

    for index, set in ipairs(left) do
        local other = right[index]
        if set.name ~= other.name or set.class ~= other.class then return false end

        for _, slot in ipairs(SLOTS) do
            if not same_entry(set.slots[slot], other.slots[slot]) then return false end
        end
    end

    return true
end

-- Never write text that does not read back as the same sets.
local function serialize(sets)
    local text  = M.write_sets(sets)
    local data  = M.parse(text)
    local again = data and M.read_sets(data)
    if again == nil or not same_sets(sets, again) then return nil, 'invalid-sets' end

    return text
end

function M.new_memory_store()
    local records = {}
    return
    {
        records = records,
        load = function(name)
            if records[name] == nil then return 'not_found' end

            return 'ok', records[name]
        end,
        save = function(name, text)
            records[name] = text
            return 'ok'
        end,
    }
end

-- Every call names the profile this store was built for; the host refuses
-- it once another character's profile is current.
local function new_native_store(profile)
    return
    {
        load = function(name) return bahamut.record_load(name, profile) end,
        save = function(name, text) return bahamut.record_save(name, text, profile) end,
    }
end

-- Without a verified profile, the memory store keeps sets only for this session.
function M.new_manager(store, persistent)
    local manager =
    {
        store = store,
        persistent = persistent,
        sets = {},
        -- A file that cannot be read stays untouched: every change is refused.
        problem = nil,
        text = nil,
        generation = 0,
    }
    return setmetatable(manager, Manager)
end

function Manager:load()
    local status, text = self.store.load(SETS_RECORD)
    self.generation    = self.generation + 1
    self.sets          = {}
    self.problem       = nil
    self.text          = nil
    if status == 'not_found' then return true end

    if status ~= 'ok' then
        self.problem = 'sets.lua: ' .. tostring(status)
        return false
    end

    local data, problem = M.parse(text)
    local sets
    if data ~= nil then sets, problem = M.read_sets(data) end

    if sets == nil then
        self.problem = 'sets.lua ' .. problem
        return false
    end

    self.sets = sets
    self.text = text
    return true
end

function Manager:changed_on_disk()
    local status, text = self.store.load(SETS_RECORD)
    if status == 'not_found' then return self.text ~= nil end

    return status ~= 'ok' or text ~= self.text
end

-- Change memory only after the store reports success.
function Manager:commit(sets)
    if self.problem ~= nil then return nil, 'file-unreadable' end

    local text, reason = serialize(sets)
    if text == nil then return nil, reason end

    local status = self.store.save(SETS_RECORD, text)
    if status ~= 'ok' then return nil, 'store-' .. tostring(status) end

    self.sets       = sets
    self.text       = text
    self.generation = self.generation + 1
    return true
end

local function copy_list(sets)
    local copy = {}
    for index, set in ipairs(sets) do copy[index] = set end

    return copy
end

function M.abbreviation(snapshot)
    local entry = JOBS[snapshot.job_id or 0] or CLASSES[snapshot.class_id or 0]
    return entry and entry[1] or ''
end

-- Every slot, each an exact copy. Unknown slots are never saved as Empty.
local function capture(snapshot)
    if type(snapshot) ~= 'table' or snapshot.complete ~= true or type(snapshot.slots) ~= 'table' then return nil, 'incomplete-snapshot' end

    local by_slot = {}
    for _, row in ipairs(snapshot.slots) do by_slot[row.slot] = row end

    local slots = {}
    for _, slot in ipairs(SLOTS) do
        local row = by_slot[slot]
        if row == nil then return nil, 'incomplete-snapshot' end

        if row.state == 'empty' then
            slots[slot] = { state = 'empty' }
        elseif row.state == 'item' and is_valid_uid(row.uid) and whole(row.catalog, 4294967295) and row.catalog > 0 then
            slots[slot] = { state = 'item', catalog = row.catalog, uid = row.uid }
        else
            return nil, 'incomplete-snapshot'
        end
    end

    return slots
end

-- Set<n> after the new set's number, skipping names already in use.
local function default_name(sets)
    local taken = {}
    for _, set in ipairs(sets) do taken[string.lower(set.name)] = true end

    local number = #sets + 1
    while taken['set' .. number] do number = number + 1 end

    return 'Set' .. number
end

function Manager:save_current(name, snapshot)
    if #self.sets >= MAX_SETS then return nil, 'limit' end

    local slots, reason = capture(snapshot)
    if slots == nil then return nil, reason end

    local class = M.abbreviation(snapshot)
    name        = name or default_name(self.sets)
    if not is_valid_name(name) then return nil, 'bad-name' end

    local sets        = copy_list(self.sets)
    sets[#sets + 1]   = { name = name, class = class, slots = slots }
    local ok, problem = self:commit(sets)
    if not ok then return nil, problem end

    return #sets
end

function Manager:update_from_current(index, snapshot)
    local prior = self.sets[index]
    if prior == nil then return nil, 'not-found' end

    local slots, reason = capture(snapshot)
    if slots == nil then return nil, reason end

    local sets        = copy_list(self.sets)
    sets[index]       = { name = prior.name, class = M.abbreviation(snapshot), slots = slots }
    local ok, problem = self:commit(sets)
    if not ok then return nil, problem end

    return index
end

function Manager:rename(index, name)
    local prior = self.sets[index]
    if prior == nil then return nil, 'not-found' end

    if not is_valid_name(name) then return nil, 'bad-name' end

    local sets        = copy_list(self.sets)
    sets[index]       = { name = name, class = prior.class, slots = prior.slots }
    local ok, problem = self:commit(sets)
    if not ok then return nil, problem end

    return index
end

function Manager:move(index, position)
    if self.sets[index] == nil then return nil, 'not-found' end

    if not whole(position, #self.sets) or position < 1 then return nil, 'bad-position' end

    local sets = copy_list(self.sets)
    table.insert(sets, position, table.remove(sets, index))
    local ok, problem = self:commit(sets)
    if not ok then return nil, problem end

    return position
end

function Manager:delete(index)
    if self.sets[index] == nil then return nil, 'not-found' end

    local sets = copy_list(self.sets)
    table.remove(sets, index)
    local ok, problem = self:commit(sets)
    if not ok then return nil, problem end

    return index
end

-- A set number, or a set name compared without case. Names may repeat.
function Manager:find(reference)
    local number = tonumber(reference)
    if number ~= nil then return self.sets[number] and number or nil end

    local wanted = string.lower(reference)
    local found
    for index, set in ipairs(self.sets) do
        if string.lower(set.name) == wanted then
            if found ~= nil then return nil, 'ambiguous-name' end

            found = index
        end
    end

    return found
end

local function current_slots(snapshot)
    if type(snapshot) ~= 'table' or snapshot.complete ~= true or type(snapshot.slots) ~= 'table' then return nil end

    local current = {}
    for _, row in ipairs(snapshot.slots) do current[row.slot] = row end

    return current
end

-- Every slot the set names is worn: the exact copy when it names one.
function M.matches(set, snapshot)
    local current = current_slots(snapshot)
    if current == nil then return false end

    local named = false
    for _, slot in ipairs(SLOTS) do
        local entry = set.slots[slot]
        local now   = current[slot]
        if entry ~= nil then
            named = true
            if now == nil then return false end

            if entry.state == 'empty' then
                if now.state ~= 'empty' then return false end
            elseif now.state ~= 'item' or now.catalog ~= entry.catalog or (entry.uid ~= nil and now.uid ~= entry.uid) then
                return false
            end
        end
    end

    return named
end

-- Omitted slots keep their gear. An item without a copy uses the matching
-- instance worn in that slot, or else the only copy carried.
function M.intent(set, snapshot, find_copy)
    local current = current_slots(snapshot)
    if current == nil then return nil, 'incomplete-snapshot' end

    local slots = {}
    for _, slot in ipairs(SLOTS) do
        local entry = set.slots[slot]
        local now   = current[slot]
        if now == nil or now.state == 'unknown' then return nil, 'incomplete-snapshot' end

        if entry == nil then
            slots[slot] = now.state == 'item' and { state = 'item', uid = now.uid } or { state = 'empty' }
        elseif entry.state == 'empty' then
            slots[slot] = { state = 'empty' }
        elseif entry.uid ~= nil then
            slots[slot] = { state = 'item', uid = entry.uid }
        elseif now.state == 'item' and now.catalog == entry.catalog then
            slots[slot] = { state = 'item', uid = now.uid }
        else
            local state, uid = find_copy(entry.catalog)
            if state ~= 'available' then return nil, SLOT_KEYS[slot] .. ' ' .. item_label(entry) .. ': ' .. tostring(state) end

            slots[slot] = { state = 'item', uid = uid }
        end
    end

    return { slots = slots }
end

M.slots      = SLOTS
M.slot_keys  = SLOT_KEYS
M.max_sets   = MAX_SETS
M.manager    = nil
M.profile    = nil
M.session    = nil
M.selected   = 0
M.visible    = false
M.message    = ''
M.equipping  = false
M.equip_name = ''

-- Sets persist only under the verified server and character profile key
-- from the host. Without one, sets live in memory only.
local function rebuild(profile)
    local manager
    if profile ~= nil then
        manager = M.new_manager(new_native_store(profile), true)
    else
        manager = M.new_manager(M.new_memory_store(), false)
    end

    manager:load()
    M.manager  = manager
    M.profile  = profile
    M.selected = 0
    M.session  = bahamut.equipment_session()
end

function M.status()
    return { persistent = M.manager ~= nil and M.manager.persistent, profile = M.profile, problem = M.manager and M.manager.problem }
end

-- Debug commands pass through without bahamut.debug_equip and bahamut.debug_equip_status.
-- Bag positions are 0-based; the game's Equip command slots are 1-based.
-- Only commands print results: chat_print requires the game's command thread.
function M.describe_equip(status)
    local text = string.format('gearsets: debug equip %d %s (%s) bag=%d slot=%d invocations=%d', status.id, status.phase, status.reason, status.bag_slot, status.equip_slot, status.invocations)
    if status.uid ~= nil then text = text .. ' uid=' .. status.uid end

    if status.return_value ~= nil then text = text .. ' returned=' .. tostring(status.return_value) end

    return text
end

local function debug_equip(name)
    local bag, slot, uid = string.match(name, '^/gearsets debug%-equip (%d+) (%d+) (%x+)$')
    if bag == nil then bag, slot = string.match(name, '^/gearsets debug%-equip (%d+) (%d+)$') end

    if bag == nil then return 'usage: /gearsets debug-equip <bag slot> <equip slot> [uid]' end

    local ok, value = bahamut.debug_equip(tonumber(bag), tonumber(slot), uid and string.lower(uid) or nil)
    if ok then return 'gearsets: debug equip ' .. number_text(value) .. ' queued' end

    return 'gearsets: debug equip refused: ' .. tostring(value)
end

gearsets = M

function load()
    rebuild(bahamut.profile_key())
end

local function refresh()
    local profile = bahamut.profile_key()
    local session = bahamut.equipment_session()
    -- A character change always starts a new session. Within one session a
    -- missing key is a passing read gap: keep the manager, whose store calls
    -- the host refuses until the same key returns.
    if M.manager ~= nil and session == M.session and profile == nil and M.profile ~= nil then return end

    if M.manager == nil or profile ~= M.profile or M.session ~= session then
        bahamut.gearsets_cancel()
        rebuild(profile)
    elseif M.manager.problem == 'sets.lua: unavailable' then
        -- The host refused a read during a key gap; read again once it returns.
        M.manager:load()
    end
end

-- Equipment from a later session must not be written into this manager's
-- sets; capture reports a nil snapshot as incomplete.
local function current_snapshot()
    local snapshot = bahamut.equipment_snapshot()
    if snapshot ~= nil and snapshot.session ~= M.session then return nil end

    return snapshot
end

local function result_text()
    local result = bahamut.gearsets_result()
    -- The transaction's initial state is rejected with no reason.
    if result == nil or (result.phase == 'rejected' and result.reason == '') then return 'no equip result' end

    local text = result.phase
    if result.reason ~= '' then text = text .. ' (' .. result.reason .. ')' end

    if result.slot ~= nil then text = text .. ' slot=' .. (SLOT_KEYS[result.slot] or result.slot) end

    if #result.changed > 0 then text = text .. ' changed=' .. table.concat(result.changed, ',') end

    return text .. ' submissions=' .. result.submissions
end

-- The window shows progress and problems; a completed equip needs no message.
local function equip_status()
    local result = bahamut.gearsets_result()
    if result == nil or result.phase == 'complete' or (result.phase == 'rejected' and result.reason == '') then return '' end

    if result.phase == 'checking' or result.phase == 'applying' then return 'Equipping ' .. M.equip_name .. '...' end

    local text = 'Equip ' .. result.phase
    if result.reason ~= '' then text = text .. ': ' .. result.reason end

    if result.slot ~= nil and SLOT_KEYS[result.slot] then text = text .. ' (' .. SLOT_KEYS[result.slot] .. ')' end

    return text
end

function update(delta_seconds)
    refresh()
    if M.equipping then
        local result = bahamut.gearsets_result()
        M.message    = equip_status()
        M.equipping  = result ~= nil and (result.phase == 'checking' or result.phase == 'applying')
    end
end

local function equip(index)
    local set = M.manager.sets[index]
    if set == nil then return nil, 'not-found' end

    local intent, reason = M.intent(set, current_snapshot(), bahamut.find_item_copy)
    if intent == nil then return nil, reason end

    local accepted, problem = bahamut.gearsets_equip(intent)
    if not accepted then return nil, tostring(problem) end

    M.equipping  = true
    M.equip_name = set.name
    return index
end

-- Returns the affected set number or nil and a reason. Window edits refuse
-- if the file changed on disk and reload the list to preserve hand edits.
function M.act(kind, index, text, from_window)
    local profile, session = M.profile, M.session
    refresh()
    if profile ~= M.profile or session ~= M.session then return nil, 'session-changed' end

    if kind == 'cancel' then
        bahamut.gearsets_cancel()
        return 0
    end

    if kind == 'equip' then return equip(index) end

    if from_window and M.manager:changed_on_disk() then
        M.manager:load()
        return nil, 'sets.lua changed and was reloaded'
    end

    local manager = M.manager
    if kind == 'save' then return manager:save_current(text, current_snapshot()) end

    if kind == 'update' then return manager:update_from_current(index, current_snapshot()) end

    if kind == 'rename' then return manager:rename(index, text) end

    if kind == 'delete' then return manager:delete(index) end

    if kind == 'move' then return manager:move(index, text == 'up' and index - 1 or text == 'down' and index + 1 or tonumber(text)) end

    return nil, 'unknown-action'
end

local function window_action(action)
    if action.kind == 'close' then
        M.visible = false
        return
    end

    if action.kind == 'select' then
        M.selected = action.row
        return
    end

    local result, reason = M.act(action.kind, action.row, action.text ~= '' and action.text or nil, true)
    if result == nil then
        M.message = action.kind .. ' failed: ' .. tostring(reason)
        return
    end

    if action.kind ~= 'equip' and action.kind ~= 'cancel' then M.message = '' end

    if action.kind == 'save' or action.kind == 'move' then
        M.selected = result
    elseif action.kind == 'delete' then
        M.selected = 0
    end
end

function draw()
    refresh()
    if not M.visible then return end

    local snapshot = current_snapshot()
    local rows     = {}
    for _, set in ipairs(M.manager.sets) do
        rows[#rows + 1] = { name = set.name, abbreviation = set.class, current = M.matches(set, snapshot) }
    end

    if M.selected > #rows then M.selected = 0 end

    local action = bahamut.gearsets_window(
        {
            rows = rows,
            selected = M.selected,
            status = M.manager.problem or M.message,
            equipping = M.equipping,
            editable = M.manager.problem == nil,
            context = (M.profile or '') .. '|' .. tostring(M.session) .. '|' .. M.manager.generation,
        })
    if action ~= nil and action.kind ~= '' then window_action(action) end
end

local HELP =
{
    '/gs (or /gearsets) opens or closes Gear Sets. Commands, where <set> is a number or name:',
    'list; save [name]; equip|update|delete <set>; rename <number> <name>;',
    'move <number> <position>; cancel; status; reload.',
    'Sets are in config/addons/gearsets/sets/ and can be edited by hand.',
}

local function reply(result, reason, done)
    bahamut.chat_print('gearsets: ' .. (result ~= nil and done or tostring(reason)))
end

function command(name)
    refresh()
    if name == '/gs' or string.sub(name, 1, 4) == '/gs ' then name = '/gearsets' .. string.sub(name, 4) end

    if name == '/gearsets' then
        M.visible = not M.visible
        if M.visible then M.manager:load() end

        return true
    end

    if string.sub(name, 1, 22) == '/gearsets debug-equip ' or name == '/gearsets debug-status' then
        if bahamut.debug_equip == nil or bahamut.debug_equip_status == nil then return false end

        if name == '/gearsets debug-status' then
            local status = bahamut.debug_equip_status()
            bahamut.chat_print(status and M.describe_equip(status) or 'gearsets: no debug equip')
        else
            bahamut.chat_print(debug_equip(name))
        end

        return true
    end

    local verb, rest = string.match(name, '^/gearsets (%S+)%s*(.*)$')
    if verb == nil then return false end

    -- Commands that read or change sets first reload hand edits.
    if verb ~= 'status' and verb ~= 'cancel' and verb ~= 'help' then M.manager:load() end

    if M.manager.problem ~= nil and verb ~= 'help' and verb ~= 'status' and verb ~= 'cancel' and verb ~= 'equip' then
        bahamut.chat_print('gearsets: ' .. M.manager.problem)
        return true
    end

    if verb == 'help' and rest == '' then
        for _, text in ipairs(HELP) do bahamut.chat_print(text) end

        return true
    end

    if verb == 'reload' and rest == '' then
        bahamut.chat_print('gearsets: ' .. #M.manager.sets .. ' sets')
        return true
    end

    if verb == 'list' and rest == '' then
        bahamut.chat_print(M.manager.persistent and 'gearsets: sets' or 'gearsets: session-only sets')
        for index, set in ipairs(M.manager.sets) do
            bahamut.chat_print(index .. ': ' .. (set.class ~= '' and set.class .. ' ' or '') .. set.name)
        end

        return true
    end

    if verb == 'status' and rest == '' then
        bahamut.chat_print('gearsets: ' .. (M.manager.problem or result_text()))
        return true
    end

    if verb == 'cancel' and rest == '' then
        bahamut.gearsets_cancel()
        bahamut.chat_print('gearsets: cancel requested')
        return true
    end

    if verb == 'save' then
        local result, reason = M.act('save', nil, rest ~= '' and rest or nil)
        reply(result, reason, result and 'saved set ' .. result)
        return true
    end

    if verb == 'equip' or verb == 'update' or verb == 'delete' then
        if rest == '' then return false end

        local index, reason = M.manager:find(rest)
        if index == nil then
            reply(nil, reason or 'no set ' .. rest)
            return true
        end

        local result
        result, reason = M.act(verb, index)
        reply(result, reason, verb .. ' ' .. index .. (verb == 'equip' and ' started' or ': ok'))
        return true
    end

    local number, value = string.match(rest, '^(%d+)%s+(.+)$')
    if (verb == 'rename' or verb == 'move') and number ~= nil then
        local result, reason = M.act(verb, tonumber(number), value)
        reply(result, reason, verb .. ' ' .. number .. ': ok')
        return true
    end

    return false
end

function unload()
    M.manager = nil
    M.profile = nil
end
