local locked          = false
local mode            = 'dps'
local active          = false
local elapsed         = 0
local idle            = 0
local incomplete      = false
local last_dropped    = 0
local combatants      = {}
local combatant_count = 0

local class_colors    =
{
    [2] = '#FFD84D',
    [3] = '#46D9E8',
    [4] = '#E64B4B',
    [7] = '#48C774',
    [8] = '#4A8FE7',
    [22] = '#9B6BDE',
    [23] = '#F2F2F2',
}

local job_colors      =
{
    [15] = '#FFD84D',
    [16] = '#46D9E8',
    [17] = '#E64B4B',
    [18] = '#48C774',
    [19] = '#4A8FE7',
    [26] = '#9B6BDE',
    [27] = '#F2F2F2',
}

local function clear()
    active          = false
    elapsed         = 0
    idle            = 0
    incomplete      = false
    combatants      = {}
    combatant_count = 0
end

local function color_for(class_id, job_id)
    return job_colors[job_id] or class_colors[class_id] or '#A0A0A0'
end

local function actor_name(id, name)
    if name and name ~= '' then return name end

    return string.format('Actor %08X', id)
end

local function combatant(id, name, class_id, job_id)
    local row = combatants[id]
    if row then
        if name and name ~= '' then row.name = name end

        if class_id and class_id ~= 0 then row.class_id = class_id end

        if job_id and job_id ~= 0 then row.job_id = job_id end

        row.color = color_for(row.class_id, row.job_id)
        return row
    end

    if combatant_count >= 64 then
        incomplete = true
        return nil
    end

    row             =
    {
        id = id,
        name = actor_name(id, name),
        class_id = class_id or 0,
        job_id = job_id or 0,
        color = color_for(class_id, job_id),
        damage = 0,
        healing = 0,
        attempts = 0,
        hits = 0,
    }
    combatants[id]  = row
    combatant_count = combatant_count + 1
    return row
end

local function nonnegative_integer(value)
    if type(value) ~= 'number' or value ~= value or value == math.huge then
        return 0
    end

    return math.floor(math.max(0, value))
end

local function observe(event)
    local kind = event.kind
    if kind ~= 'damage' and kind ~= 'miss' and kind ~= 'healing' then return false end

    local row = combatant(event.source_id, event.source_name,
        event.source_class_id, event.source_job_id)
    if not row then return false end

    if kind == 'damage' then
        row.attempts = row.attempts + 1
        row.hits     = row.hits + 1
        row.damage   = row.damage + nonnegative_integer(event.amount)
    elseif kind == 'miss' then
        row.attempts = row.attempts + 1
    elseif kind == 'healing' then
        row.healing = row.healing + nonnegative_integer(event.amount)
    end

    active = true
    return true
end

local function rate(row, metric)
    return row[metric] / math.max(elapsed, 1)
end

local function accuracy(row)
    if row.attempts == 0 then return '--' end

    return string.format('%.1f%%', 100 * row.hits / row.attempts)
end

local function ranked_rows()
    local metric = mode == 'dps' and 'damage' or 'healing'
    local rows   = {}
    for _, row in pairs(combatants) do
        if row[metric] > 0 then rows[#rows + 1] = row end
    end

    table.sort(rows, function(a, b)
        local a_rate = rate(a, metric)
        local b_rate = rate(b, metric)
        if a_rate ~= b_rate then return a_rate > b_rate end

        return a.id < b.id
    end)

    local display = {}
    for index = 1, math.min(#rows, 5) do
        local row             = rows[index]
        display[#display + 1] =
        {
            name = row.name,
            amount = row[metric],
            rate = rate(row, metric),
            accuracy = accuracy(row),
            color = row.color,
        }
    end

    return display
end

function load()
    locked = bahamut.settings_get('locked', 'false') == 'true'
    mode   = bahamut.settings_get('mode', 'dps')
    if mode ~= 'dps' and mode ~= 'hps' then mode = 'dps' end

    local _, dropped = bahamut.combat_events()
    last_dropped     = dropped
end

function command(name)
    if name == '/combatparser mode' then
        mode = mode == 'dps' and 'hps' or 'dps'
        bahamut.settings_set('mode', mode)
        bahamut.chat_print('Combat Parser mode: ' .. string.upper(mode))
        return true
    end

    if name == '/combatparser' or name == '/combatparser help' then
        bahamut.chat_print('/combatparser mode')
        bahamut.chat_print('/combatparser reset')
        bahamut.chat_print('/combatparser lock')
        return true
    end

    if name == '/combatparser lock' then
        locked = not locked
        bahamut.settings_set('locked', locked and 'true' or 'false')
        bahamut.chat_print(locked and 'Combat Parser overlay locked' or 'Combat Parser overlay unlocked')
        return true
    end

    if name == '/combatparser reset' then
        clear()
        local _, dropped = bahamut.combat_events()
        last_dropped     = dropped
        bahamut.chat_print('Combat Parser reset')
        return true
    end

    return false
end

function update(dt)
    local events, dropped = bahamut.combat_events()
    if dropped > last_dropped then incomplete = true end

    last_dropped      = dropped

    local had_results = active
    local gap         = active and idle + math.max(0, dt) or 0
    local observed    = false
    for _, event in ipairs(events) do
        if observe(event) then observed = true end
    end

    if observed then
        if had_results and gap <= 10 then elapsed = elapsed + gap end

        idle = 0
    else
        idle = gap
    end
end

function draw()
    bahamut.combat_meter(mode, ranked_rows(), locked, incomplete)
end
