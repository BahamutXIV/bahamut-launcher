-- gearsets_addon_tests appends this to addons/gearsets/gearsets.lua for real host bindings.
-- Each update advances one step; C++ changes the profile, equipment snapshot and disk files between calls.
-- Results go to the settings key `result`.

local failures = {}
local seen     = {}

local function check(condition, message)
    if not condition then failures[#failures + 1] = message end
end

local function finish()
    if #failures == 0 then
        bahamut.settings_set('result', 'ok')
    else
        bahamut.settings_set('result', 'fail: ' .. string.sub(table.concat(failures, ' | '), 1, 900))
    end
end

local function names_of(manager)
    local names = {}
    for _, set in ipairs(manager.sets) do
        names[#names + 1] = set.name
    end

    return table.concat(names, ',')
end

local WIDE_UID    = 'fedcba9876543210'

local steps       =
{
    -- 1: persistent profile-a; save two sets from the native snapshot.
    function()
        local manager = gearsets.manager
        check(gearsets.status().persistent == true and gearsets.status().profile == 'profile-a', 'profile-a should be persistent')
        check(#manager.sets == 0 and manager.problem == nil, 'profile-a should start with no sets file')
        local snapshot = bahamut.equipment_snapshot()
        check(snapshot ~= nil and snapshot.complete == true and type(snapshot.generation) == 'number', 'the native snapshot should be complete')
        check(#snapshot.slots == 18 and snapshot.class_id == 3 and snapshot.job_id == nil, 'the snapshot should carry the class and no job')
        check(snapshot.slots[8].slot == 10 and snapshot.slots[8].uid == WIDE_UID and snapshot.slots[8].catalog == 8030423, 'a wide instance id should arrive as text')
        check(snapshot.slots[2].slot == 1 and snapshot.slots[2].state == 'empty' and snapshot.slots[2].uid == nil, 'an Empty slot should carry no item')
        seen.first_generation = snapshot.generation
        check(manager:save_current('Gladiator - Tank', snapshot) == 1, 'the first set should save')
        check(manager:save_current(nil, snapshot) == 2 and manager.sets[2].name == 'Set2', 'the second set should take the default name')
    end,
    -- 2: the saved set matches the unchanged equipment.
    function()
        local set = gearsets.manager.sets[1]
        check(gearsets.matches(set, bahamut.equipment_snapshot()), 'unchanged equipment should match its set')
        check(set.slots[10].uid == WIDE_UID and set.slots[1].state == 'empty', 'the saved set should hold the exact copy and the empty slot')
    end,
    -- 3: another profile sees none of profile-a's sets.
    function()
        check(gearsets.status().profile == 'profile-b' and #gearsets.manager.sets == 0, 'profile-b should start empty')
        check(bahamut.record_load('sets', 'profile-b') == 'not_found', 'profile-b should not read profile-a sets')
        check(bahamut.record_load('sets', 'profile-a') == 'unavailable' and bahamut.record_list('profile-a') == 'unavailable'
            and bahamut.record_save('sets', 'x', 'profile-a') == 'unavailable' and bahamut.record_delete('sets', 'profile-a') == 'unavailable',
            'a store bound to profile-a should be refused while profile-b is current')
        check(bahamut.record_load('sets') == 'unavailable', 'a record call without its profile should be refused')
        check(gearsets.manager:save_current('B set', bahamut.equipment_snapshot()) == 1, 'profile-b should save its own file')
    end,
    -- 4: no verified profile: session-only, and the record calls refuse.
    function()
        check(gearsets.status().persistent == false and gearsets.status().profile == nil, 'without a profile the addon should be session-only')
        check(bahamut.profile_key() == nil, 'no profile key should be reported')
        check(bahamut.record_save('sets', 'x') == 'unavailable', 'saving without a profile should be refused')
        check(bahamut.record_load('sets') == 'unavailable', 'loading without a profile should be refused')
        check(bahamut.record_list() == 'unavailable', 'listing without a profile should be refused')
        check(gearsets.manager:save_current('Session only', bahamut.equipment_snapshot()) == 1, 'a session set should save in memory')
    end,
    -- 5: back on profile-a.
    function()
        local manager = gearsets.manager
        check(gearsets.status().persistent == true and gearsets.status().profile == 'profile-a', 'profile-a should be selected again')
        check(names_of(manager) == 'Gladiator - Tank,Set2', 'profile-a should list its own sets, got ' .. names_of(manager))
    end,
    -- 6: the equipment now holds another copy of one item type.
    function()
        local snapshot = bahamut.equipment_snapshot()
        check(snapshot.generation > seen.first_generation, 'a changed snapshot should have a new generation')
        check(not gearsets.matches(gearsets.manager.sets[1], snapshot), 'another copy should not match the saved copy')
    end,
    -- 7: an unreadable slot cannot be saved.
    function()
        local snapshot = bahamut.equipment_snapshot()
        check(snapshot.complete == false, 'the snapshot should be incomplete')
        local index, reason = gearsets.manager:save_current('Unreadable', snapshot)
        check(index == nil and reason == 'incomplete-snapshot', 'an incomplete snapshot should not save')
    end,
    -- 8: a new set joins the existing file.
    function()
        check(gearsets.manager:save_current('After', bahamut.equipment_snapshot()) == 3, 'the new set should take position 3')
        seen.manager = gearsets.manager
    end,
    -- 9: the key is briefly missing in the same session.
    function()
        check(gearsets.manager == seen.manager and gearsets.status().profile == 'profile-a', 'a read gap should keep the profile-a manager')
        local index, reason = gearsets.manager:save_current('Gap', bahamut.equipment_snapshot())
        check(index == nil and reason == 'store-unavailable', 'saving during a read gap should be refused, got ' .. tostring(reason))
        check(#gearsets.manager.sets == 3, 'a refused save should not add a set')
    end,
    -- 10: the same key returns.
    function()
        check(gearsets.manager == seen.manager and names_of(gearsets.manager) == 'Gladiator - Tank,Set2,After', 'the returning key should keep the same manager and sets')
        finish()
    end,
}

local step_index  = 0
local real_load   = load
local real_update = update

function load()
    real_load()
    if bahamut.settings_get('phase', 'run') == 'verify' then
        -- A new host over the same state root: everything must come back.
        local manager = gearsets.manager
        check(gearsets.status().persistent == true and gearsets.status().profile == 'profile-a', 'the restarted addon should use profile-a')
        check(names_of(manager) == 'Gladiator - Tank,Set2,After', 'the restarted addon should list the saved sets, got ' .. names_of(manager))
        check(manager.sets[1].slots[10].uid == WIDE_UID and manager.sets[1].slots[10].catalog == 8030423 and manager.sets[1].class == 'GLD', 'the saved equipment should round trip')
        check(manager.sets[1].slots[1].state == 'empty', 'an Empty slot should round trip')
        check(manager.sets[1].slots[0].uid == '0000000007af7850' and manager.sets[1].slots[0].catalog == 4030013, 'a short instance id should round trip')
        finish()
    end
end

function update(delta_seconds)
    real_update(delta_seconds)
    step_index = step_index + 1
    if steps[step_index] then
        local ok, err = pcall(steps[step_index])
        if not ok then
            failures[#failures + 1] = 'step ' .. step_index .. ': ' .. tostring(err)
            finish()
        end
    end
end
