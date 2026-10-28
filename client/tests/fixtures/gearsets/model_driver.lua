-- Appended to addons/gearsets/gearsets.lua by gearsets_addon_tests. It exercises
-- the sets file model against in-memory stores and reports through the
-- settings key `result`.

local failures = {}

local function check(condition, message)
    if not condition then failures[#failures + 1] = message end
end

local WIDE_UID  = 'fedcba9876543210'
local COPY      = '0000000007af7850'
-- Item type IDs and their names in client/src/item_names.inc.
local CANE      = 5030101 -- WEATHERED_CANE
local HORA      = 4020001 -- WEATHERED_HORA
local DALMATICA = 8030423 -- COTTON_DALMATICA
local RING      = 9050052 -- BONE_RING

local function lines(...)
    return table.concat({ ... }, '\n') .. '\n'
end

-- A complete snapshot with chosen items. Slots not listed are Empty.
local function snapshot_of(items, overrides)
    local rows = {}
    for _, slot in ipairs(gearsets.slots) do
        local item = items[slot]
        if item then
            rows[#rows + 1] = { slot = slot, state = 'item', uid = item[1], catalog = item[2] }
        else
            rows[#rows + 1] = { slot = slot, state = 'empty' }
        end
    end

    local snapshot = { complete = true, generation = 1, class_id = 3, slots = rows }
    for key, value in pairs(overrides or {}) do
        snapshot[key] = value
    end

    return snapshot
end

local function deep_equal(left, right)
    if type(left) ~= type(right) then return false end

    if type(left) ~= 'table' then return left == right end

    for key, value in pairs(left) do
        if not deep_equal(value, right[key]) then return false end
    end

    for key in pairs(right) do
        if left[key] == nil then return false end
    end

    return true
end

-- A manager over a memory store, optionally holding a file already.
local function new_manager(text)
    local store        = gearsets.new_memory_store()
    store.records.sets = text
    local manager      = gearsets.new_manager(store, true)
    local loaded       = manager:load()
    return manager, store, loaded
end

local function names_of(manager)
    local names = {}
    for _, set in ipairs(manager.sets) do names[#names + 1] = set.name end

    return table.concat(names, ',')
end

local function test_parse_accepts_literals()
    local text      = lines(
        '-- header comment',
        '--[[ block',
        'comment ]]',
        'return {',
        "    { name = 'Tank', class = 'gld', main = 5030101; body = { 'WEATHERED_HORA', copy = 'FEDCBA9876543210' }, off = 'empty' },",
        '    { name = "Second", [ "head" ] = "WEATHERED_CANE", legs = "empty" },',
        '}',
        '-- trailing comment')
    local data, err = gearsets.parse(text)
    check(data ~= nil, 'a literal sets file should parse: ' .. tostring(err))
    if data == nil then return end

    check(#data == 2 and data[1].name == 'Tank' and data[1].main == CANE and data[1].body[1] == 'WEATHERED_HORA' and data[2].head == 'WEATHERED_CANE' and data[2].legs == 'empty',
        'parsed values should match the file')
    local sets, problem = gearsets.read_sets(data)
    check(sets ~= nil, 'the parsed file should read as sets: ' .. tostring(problem))
    if sets == nil then return end

    check(sets[1].class == 'GLD' and sets[1].slots[0].catalog == CANE and sets[1].slots[0].uid == nil, 'an item number is an item-type-only slot')
    check(sets[1].slots[10].catalog == HORA and sets[1].slots[10].uid == WIDE_UID, 'a copy is kept as lowercase hex')
    check(sets[1].slots[1].state == 'empty' and sets[1].slots[8] == nil, 'empty is explicit and a slot left out stays out')
end

local function test_parse_refuses_code()
    local cases =
    {
        { 'a function call',     lines('return {', '  {', '    name = print("x"),', '  },', '}'),                        'line 3: expected a value' },
        { 'a global reference',  lines('return {', '  {', "    name = 'A',", '    head = WEATHERED_CANE,', '  },', '}'), 'line 4: expected a value' },
        { 'a function value',    lines('return {', '  { name = function() end },', '}'),                                 'line 2: expected a value' },
        { 'a concatenation',     lines('return {', "  { name = 'a' .. 'b' },", '}'),                                     "line 2: expected ',' or '}'" },
        { 'code before return',  lines('-- comment', 'x = 1', 'return {}'),                                              "line 2: expected 'return'" },
        { 'code after the sets', lines('return {}', '', 'os.exit()'),                                                    'line 3: unexpected text after the sets' },
        { 'a fraction',          lines('return {', '  { name = "a", main = 1.5 },', '}'),                                'line 2: only whole numbers are supported' },
        { 'a duplicate field',   lines('return {', "  { name = 'a', name = 'b' },", '}'),                                'line 2: duplicate field name' },
        { 'a bad escape',        lines('return {', "  { name = 'a\\q' },", '}'),                                         'line 2: unsupported escape' },
        { 'a broken string',     lines('return {', "  { name = 'a", '  },', '}'),                                        'line 2: unfinished string' },
        { 'deep nesting',        'return { { { { { {} } } } } }',                                                        'line 1: tables nest too deeply' },
    }
    for _, case in ipairs(cases) do
        local data, err = gearsets.parse(case[2])
        check(data == nil and err == case[3], case[1] .. ' should be refused with "' .. case[3] .. '", got ' .. tostring(err))
    end

    -- Nothing in the file is ever executed.
    ran             = nil
    local data, err = gearsets.parse(lines('return {', '  { name = (function() ran = true end)() },', '}'))
    check(data == nil and ran == nil and string.sub(err or '', 1, 7) == 'line 2:', 'a function call must be refused, not run')
    data = gearsets.parse(lines('ran = true', 'return {}'))
    check(data == nil and ran == nil, 'a statement before return must be refused, not run')
    ran                          = nil

    -- A refused file leaves the manager unwritable and the file untouched.
    local text                   = lines('return {', '  {', '    name = os.time(),', '  },', '}')
    local manager, store, loaded = new_manager(text)
    check(loaded == false and manager.problem == 'sets.lua line 3: expected a value' and #manager.sets == 0, 'an unparseable file should be reported with its line')
    local index, reason = manager:save_current('Nope', snapshot_of({}))
    check(index == nil and reason == 'file-unreadable', 'an unreadable file should refuse a save, got ' .. tostring(reason))
    check(select(2, manager:commit({})) == 'file-unreadable' and store.records.sets == text, 'an unreadable file must stay byte for byte')
end

local function test_read_sets_refuses_bad_data()
    local cases =
    {
        { "name = 'A', class = 'XYZ'",                                          'sets.lua set 1: unknown class XYZ' },
        { "name = 'A', cape = 'WEATHERED_CANE'",                                'sets.lua set 1: unknown field cape' },
        { "name = 'A', main = 'NO_SUCH_ITEM'",                                  'sets.lua set 1 main: unknown item NO_SUCH_ITEM' },
        { "name = 'A', main = 0",                                               'sets.lua set 1 main: unknown item 0' },
        { "name = 'A', main = { 'WEATHERED_CANE', copy = 'xyz' }",              'sets.lua set 1 main: copy must be 16 hex digits' },
        { "name = 'A', main = { 'WEATHERED_CANE', copy = '0000000000000000' }", 'sets.lua set 1 main: copy must be 16 hex digits' },
        { "name = 'A', main = { 'WEATHERED_CANE', extra = 1 }",                 'sets.lua set 1 main: unknown field extra' },
        { "class = 'GLD'",                                                      'sets.lua set 1: name must be 1-64 bytes of text' },
        { "name = '" .. string.rep('x', 65) .. "'",                             'sets.lua set 1: name must be 1-64 bytes of text' },
        { "name = 'tab\\there'",                                                'sets.lua set 1: name must be 1-64 bytes of text' },
    }
    for _, case in ipairs(cases) do
        local manager, _, loaded = new_manager(lines('return {', '  { ' .. case[1] .. ' },', '}'))
        check(loaded == false and manager.problem == case[2], 'expected "' .. case[2] .. '", got ' .. tostring(manager.problem))
    end

    local manager, _, loaded = new_manager(lines('return {', "  'not a set',", '}'))
    check(loaded == false and manager.problem == 'sets.lua set 1: not a table', 'a non-table entry should be refused')
    for _, body in ipairs({ "[3] = { name = 'Keep me' }", "[1] = { name = 'A' }, [3] = { name = 'C' }" }) do
        local sparse           = lines('return {', '  ' .. body .. ',', '}')
        local store
        manager, store, loaded = new_manager(sparse)
        check(loaded == false and manager.problem == 'sets.lua sets must be numbered without gaps' and #manager.sets == 0, 'a sparse sets table should be refused, got ' .. tostring(manager.problem))
        local index, reason = manager:save_current('Other', snapshot_of({}))
        check(index == nil and reason == 'file-unreadable' and store.records.sets == sparse, 'a sparse sets file should refuse changes and stay untouched')
    end

    manager, _, loaded = new_manager(lines('return {}'))
    check(loaded == true and #manager.sets == 0 and manager.problem == nil, 'an empty file is a valid empty list')
end

local function test_round_trip()
    local sets =
    {
        {
            name  = "O'Brien \\ Tank",
            class = 'GLD',
            slots =
            {
                [0]  = { state = 'item', catalog = CANE },
                [1]  = { state = 'empty' },
                [8]  = { state = 'item', catalog = HORA, uid = '00000000000000aa' },
                [10] = { state = 'item', catalog = DALMATICA, uid = WIDE_UID },
                [13] = { state = 'item', catalog = 123 },
            },
        },
        { name = 'Bare', class = '', slots = {} },
    }
    local text = gearsets.write_sets(sets)
    check(string.find(text, "        main = 'WEATHERED_CANE',\n", 1, true) ~= nil, 'an item-type-only slot should be written by name')
    check(string.find(text, "        head = { 'WEATHERED_HORA', copy = '00000000000000aa' },\n", 1, true) ~= nil, 'a copy should be written beside its item name')
    check(string.find(text, "        body = { 'COTTON_DALMATICA', copy = 'fedcba9876543210' },\n", 1, true) ~= nil, 'a wide copy should be written as text')
    check(string.find(text, "        off = 'empty',\n", 1, true) ~= nil, 'an empty slot should be written explicitly')
    check(string.find(text, '        hands = 123,\n', 1, true) ~= nil, 'an unnamed item should be written by number')
    check(string.find(text, '        legs =', 1, true) == nil and string.find(text, '        feet =', 1, true) == nil, 'a slot left out should not be written')
    check(string.find(text, "        name = 'O\\'Brien \\\\ Tank',\n", 1, true) ~= nil, 'quotes and backslashes in a name should be escaped')
    check(string.find(text, '-- Gear Sets for one character', 1, true) == 1 and string.find(text, '\nreturn {\n', 1, true) ~= nil, 'the file should start with the header')
    check(string.find(text, '\r', 1, true) == nil, 'the file should use plain newlines')

    local data, err = gearsets.parse(text)
    check(data ~= nil, 'the written file should parse: ' .. tostring(err))
    local back = data and gearsets.read_sets(data)
    check(back ~= nil and deep_equal(back, sets), 'the written file should read back as the same sets')
    check(back ~= nil and gearsets.write_sets(back) == text, 'writing the read sets again should give the same file')

    -- Through the manager and its store.
    local manager, store = new_manager(nil)
    local snapshot       = snapshot_of({ [0] = { COPY, CANE }, [10] = { WIDE_UID, DALMATICA } })
    check(manager:save_current('Kept', snapshot) == 1, 'a set should save')
    local fresh = gearsets.new_manager(store, true)
    check(fresh:load() == true and deep_equal(fresh.sets, manager.sets) and fresh.text == store.records.sets, 'a new manager should load what was saved')
end

local function test_save_current()
    local manager, store = new_manager(nil)
    local snapshot       = snapshot_of({ [0] = { COPY, CANE }, [10] = { WIDE_UID, DALMATICA } })
    check(manager:save_current(nil, snapshot) == 1, 'a complete snapshot should save as set 1')
    local set = manager.sets[1]
    check(set.name == 'Set1' and set.class == 'GLD', 'the name should default to Set and the set number, got ' .. tostring(set.name))
    check(deep_equal(set.slots[0], { state = 'item', catalog = CANE, uid = COPY }) and deep_equal(set.slots[10], { state = 'item', catalog = DALMATICA, uid = WIDE_UID }),
        'items should be saved as exact copies')
    local empty_slots = 0
    for _, slot in ipairs(gearsets.slots) do
        if set.slots[slot] and set.slots[slot].state == 'empty' then empty_slots = empty_slots + 1 end
    end

    check(empty_slots == 16, 'every other slot should be saved as empty, got ' .. empty_slots)
    local data = gearsets.parse(store.records.sets)
    check(data ~= nil and deep_equal(gearsets.read_sets(data), manager.sets), 'the saved file should read back')

    snapshot.job_id = 16
    check(manager:save_current(nil, snapshot) == 2 and manager.sets[2].name == 'Set2' and manager.sets[2].class == 'PLD', 'a job takes precedence over the class')
    snapshot.class_id = nil
    snapshot.job_id   = nil
    check(manager:save_current(nil, snapshot) == 3 and manager.sets[3].name == 'Set3' and manager.sets[3].class == '', 'an unknown class saves without an abbreviation')
    check(manager:save_current('Named', snapshot) == 4 and manager.sets[4].name == 'Named', 'an explicit name wins')
    local taken = new_manager(nil)
    check(taken:save_current('set2', snapshot) == 1 and taken:save_current(nil, snapshot) == 2 and taken.sets[2].name == 'Set3', 'a default name skips one already in use')

    for _, name in ipairs({ '', string.rep('x', 65), 'bad\nname', 'tab\tname', 'bad\255', '   ' }) do
        local index, reason = manager:save_current(name, snapshot)
        check(index == nil and reason == 'bad-name', 'an invalid name should be rejected')
    end

    check(manager:save_current(string.rep('x', 64), snapshot) == 5, 'a 64 byte name should be accepted')

    local unknown_slot    = snapshot_of({})
    unknown_slot.slots[3] = { slot = unknown_slot.slots[3].slot, state = 'unknown' }
    local missing_row     = snapshot_of({})
    table.remove(missing_row.slots, 5)
    local numeric_uid    = snapshot_of({})
    numeric_uid.slots[1] = { slot = 0, state = 'item', uid = 1, catalog = CANE }
    local short_uid      = snapshot_of({})
    short_uid.slots[1]   = { slot = 0, state = 'item', uid = '0001', catalog = CANE }
    local zero_uid       = snapshot_of({})
    zero_uid.slots[1]    = { slot = 0, state = 'item', uid = '0000000000000000', catalog = CANE }
    local cases          =
    {
        { 'an unknown slot',       unknown_slot },
        { 'an incomplete flag',    snapshot_of({}, { complete = false }) },
        { 'a missing slot row',    missing_row },
        { 'a numeric instance id', numeric_uid },
        { 'a short instance id',   short_uid },
        { 'a zero instance id',    zero_uid },
        { 'no snapshot',           nil },
    }
    local before         = store.records.sets
    for _, case in ipairs(cases) do
        local index, reason = manager:save_current('Rejected', case[2])
        check(index == nil and reason == 'incomplete-snapshot', case[1] .. ' should not save')
    end

    check(store.records.sets == before and #manager.sets == 5, 'rejected snapshots should write nothing')

    -- update_from_current keeps the name and position.
    local worn = snapshot_of({ [0] = { '0000000000000099', HORA } }, { class_id = 3, job_id = 16 })
    check(manager:update_from_current(2, worn) == 2, 'an update should return the set number')
    check(manager.sets[2].name == 'Set2' and manager.sets[2].class == 'PLD' and manager.sets[2].slots[0].catalog == HORA and manager.sets[2].slots[0].uid == '0000000000000099',
        'an update should keep the name and store the new equipment')
    check(select(2, manager:update_from_current(9, worn)) == 'not-found', 'an unknown set cannot be updated')

    local limited = new_manager(nil)
    for index = 1, gearsets.max_sets do
        check(limited:save_current('Set ' .. index, snapshot) == index, 'set ' .. index .. ' should save')
    end

    local over, reason = limited:save_current('Over', snapshot)
    check(over == nil and reason == 'limit', 'the set limit should be enforced')
end

local function test_failed_store_keeps_prior_file()
    local store   = gearsets.new_memory_store()
    local failing = false
    local wrapped =
    {
        load = store.load,
        save = function(name, text)
            if failing then return 'io' end

            return store.save(name, text)
        end,
    }
    local manager = gearsets.new_manager(wrapped, true)
    manager:load()
    check(manager:save_current('Keep', snapshot_of({ [0] = { COPY, CANE } })) == 1, 'setup save')
    local prior         = store.records.sets
    failing             = true
    local snapshot      = snapshot_of({})
    local index, reason = manager:save_current('New', snapshot)
    check(index == nil and reason == 'store-io' and #manager.sets == 1 and store.records.sets == prior, 'a failed save should change nothing')
    check(select(2, manager:rename(1, 'Lost')) == 'store-io' and manager.sets[1].name == 'Keep', 'a failed rename should keep the name')
    check(select(2, manager:delete(1)) == 'store-io' and #manager.sets == 1, 'a failed delete should keep the set')
    check(select(2, manager:move(1, 1)) == 'store-io', 'a failed move should report the failure')
    failing = false
    check(manager:save_current('New', snapshot) == 2 and #manager.sets == 2, 'a retry should succeed')
end

local function test_positions_names_and_edits()
    local manager, store = new_manager(nil)
    local snapshot       = snapshot_of({})
    for _, name in ipairs({ 'Alpha', 'Beta', 'Gamma', 'alpha' }) do
        manager:save_current(name, snapshot)
    end

    check(manager:find(2) == 2 and manager:find('3') == 3 and manager:find(9) == nil and manager:find('0') == nil, 'a set is found by its position')
    check(manager:find('beta') == 2 and manager:find('GAMMA') == 3 and manager:find('nobody') == nil, 'a set is found by name without case')
    local index, reason = manager:find('Alpha')
    check(index == nil and reason == 'ambiguous-name', 'a repeated name should be ambiguous')
    check(manager:find(4) == 4, 'an ambiguous name is still reachable by position')

    check(manager:rename(4, 'Delta') == 4 and names_of(manager) == 'Alpha,Beta,Gamma,Delta', 'rename by position')
    check(manager:find('alpha') == 1, 'a renamed duplicate is no longer ambiguous')
    check(select(2, manager:rename(9, 'x')) == 'not-found' and select(2, manager:rename(1, '')) == 'bad-name', 'bad renames are refused')
    check(manager:move(3, 1) == 1 and names_of(manager) == 'Gamma,Alpha,Beta,Delta', 'move to the front')
    check(manager:move(1, 4) == 4 and names_of(manager) == 'Alpha,Beta,Delta,Gamma', 'move to the back')
    check(select(2, manager:move(1, 9)) == 'bad-position' and select(2, manager:move(1, 0)) == 'bad-position' and select(2, manager:move(9, 1)) == 'not-found', 'bad moves are refused')
    check(manager:delete(2) == 2 and names_of(manager) == 'Alpha,Delta,Gamma', 'delete by position')
    check(select(2, manager:delete(9)) == 'not-found', 'an unknown set cannot be deleted')

    local fresh = gearsets.new_manager(store, true)
    fresh:load()
    check(names_of(fresh) == 'Alpha,Delta,Gamma', 'edits survive a reload')
end

local function test_changed_on_disk()
    local manager, store = new_manager(nil)
    check(manager:changed_on_disk() == false, 'a missing file is unchanged when none was read')
    manager:save_current('A', snapshot_of({}))
    check(manager:changed_on_disk() == false, 'a file just written is unchanged')
    store.records.sets = store.records.sets .. '-- hand edit\n'
    check(manager:changed_on_disk() == true, 'an edited file is changed')
    manager:load()
    check(manager:changed_on_disk() == false, 'a reloaded file is unchanged')
    store.records.sets = nil
    check(manager:changed_on_disk() == true, 'a deleted file is changed')
    local failing = gearsets.new_manager({ load = function() return 'io' end, save = function() return 'ok' end }, true)
    check(failing:load() == false and failing.problem == 'sets.lua: io' and failing:changed_on_disk() == true, 'an unreadable file is reported and counts as changed')
    check(select(2, failing:save_current('x', snapshot_of({}))) == 'file-unreadable', 'an unreadable file refuses changes')
end

local function test_matches()
    local worn = snapshot_of({ [0] = { COPY, CANE }, [10] = { WIDE_UID, DALMATICA } })
    local function set_of(slots)
        return { name = 'S', class = '', slots = slots }
    end

    check(gearsets.matches(set_of({ [0] = { state = 'item', catalog = CANE, uid = COPY } }), worn), 'the exact copy matches')
    check(not gearsets.matches(set_of({ [0] = { state = 'item', catalog = CANE, uid = '0000000000000001' } }), worn), 'another copy of the item does not match')
    check(gearsets.matches(set_of({ [0] = { state = 'item', catalog = CANE } }), worn), 'an item-type-only slot matches any copy')
    check(not gearsets.matches(set_of({ [0] = { state = 'item', catalog = HORA } }), worn), 'another item does not match')
    check(gearsets.matches(set_of({ [1] = { state = 'empty' } }), worn) and not gearsets.matches(set_of({ [0] = { state = 'empty' } }), worn), 'empty matches only an empty slot')
    check(gearsets.matches(set_of({ [0] = { state = 'item', catalog = CANE }, [10] = { state = 'item', catalog = DALMATICA, uid = WIDE_UID }, [1] = { state = 'empty' } }), worn),
        'every named slot must match')
    check(not gearsets.matches(set_of({}), worn), 'a set naming no slot is never current')
    local unknown    = snapshot_of({ [0] = { COPY, CANE } })
    unknown.slots[2] = { slot = 1, state = 'unknown' }
    check(not gearsets.matches(set_of({ [1] = { state = 'empty' } }), unknown), 'an unreadable slot does not match')
    check(
        not gearsets.matches(set_of({ [0] = { state = 'item', catalog = CANE } }), snapshot_of({}, { complete = false })) and
        not gearsets.matches(set_of({ [0] = { state = 'item', catalog = CANE } }), nil),
        'no complete snapshot matches nothing')
end

local function test_intent()
    local worn    = snapshot_of(
        {
            [0]  = { COPY, 4030013 },
            [10] = { WIDE_UID, DALMATICA },
            [13] = { '00000000000000c1', 7777 },
            [21] = { '00000000000000a1', RING },
            [22] = { '00000000000000a2', RING },
        })
    local answers =
    {
        [CANE]    = { 'available', '00000000000000b1' },
        [HORA]    = { 'ambiguous' },
        [4020002] = { 'missing' },
        [4020003] = { 'unavailable' },
    }
    local asked   = {}
    local function find_copy(catalog)
        asked[#asked + 1] = catalog
        local answer      = answers[catalog]
        return answer[1], answer[2]
    end

    local set            =
    {
        name = 'Mix',
        class = '',
        slots =
        {
            [0]  = { state = 'item', catalog = CANE },
            [1]  = { state = 'empty' },
            [10] = { state = 'item', catalog = DALMATICA },
            [21] = { state = 'item', catalog = RING },
            [22] = { state = 'item', catalog = RING, uid = '00000000000000ee' },
        },
    }
    local intent, reason = gearsets.intent(set, worn, find_copy)
    check(intent ~= nil, 'a resolvable set should give an intent: ' .. tostring(reason))
    if intent == nil then return end

    local count = 0
    for _ in pairs(intent.slots) do count = count + 1 end

    check(count == 18, 'the intent should name every slot, got ' .. count)
    check(intent.slots[0].state == 'item' and intent.slots[0].uid == '00000000000000b1', 'an item-type-only slot not worn there resolves through the lookup')
    check(intent.slots[10].uid == WIDE_UID, 'an item-type-only slot already worn there keeps that copy')
    check(intent.slots[21].uid == '00000000000000a1', 'a worn copy wins over the lookup even when the type is ambiguous')
    check(intent.slots[22].uid == '00000000000000ee', 'a named copy is requested as is')
    check(intent.slots[1].state == 'empty', 'an empty slot is requested empty')
    check(intent.slots[13].state == 'item' and intent.slots[13].uid == '00000000000000c1', 'a slot left out keeps the worn item')
    check(intent.slots[8].state == 'empty' and intent.slots[12].state == 'empty', 'a slot left out and empty stays empty')
    check(#asked == 1 and asked[1] == CANE, 'only the slot needing a lookup should ask, got ' .. #asked)

    local refusals =
    {
        { HORA,    'main WEATHERED_HORA: ambiguous' },
        { 4020002, 'main DATED_BONE_HORA: missing' },
        { 4020003, 'main DATED_TORTOISESHELL_HORA: unavailable' },
    }
    for _, case in ipairs(refusals) do
        local refused, why = gearsets.intent({ name = 'R', class = '', slots = { [0] = { state = 'item', catalog = case[1] } } }, worn, find_copy)
        check(refused == nil and why == case[2], 'expected "' .. case[2] .. '", got ' .. tostring(why))
    end

    local unknown_slot    = snapshot_of({})
    unknown_slot.slots[3] = { slot = unknown_slot.slots[3].slot, state = 'unknown' }
    for _, snapshot in ipairs({ unknown_slot, snapshot_of({}, { complete = false }) }) do
        local refused, why = gearsets.intent(set, snapshot, find_copy)
        check(refused == nil and why == 'incomplete-snapshot', 'an unreadable slot should refuse the intent')
    end

    local refused, why = gearsets.intent(set, nil, find_copy)
    check(refused == nil and why == 'incomplete-snapshot', 'no snapshot should refuse the intent')
end

function load()
    local ok, err = pcall(function()
        test_parse_accepts_literals()
        test_parse_refuses_code()
        test_read_sets_refuses_bad_data()
        test_round_trip()
        test_save_current()
        test_failed_store_keeps_prior_file()
        test_positions_names_and_edits()
        test_changed_on_disk()
        test_matches()
        test_intent()
    end)
    if not ok then failures[#failures + 1] = 'driver error: ' .. tostring(err) end

    if #failures == 0 then
        bahamut.settings_set('result', 'ok')
    else
        bahamut.settings_set('result', 'fail: ' .. string.sub(table.concat(failures, ' | '), 1, 900))
    end
end
