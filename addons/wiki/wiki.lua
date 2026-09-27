local wiki_origin         = 'https://bahamut.miraheze.org'
local maximum_query_bytes = 512

local function valid_utf8(value)
    local index = 1
    while index <= #value do
        local first              = string.byte(value, index)
        local continuation_count = 0
        local minimum            = 128
        local maximum            = 191
        if first <= 127 then
            index = index + 1
        else
            if first >= 194 and first <= 223 then
                continuation_count = 1
            elseif first >= 224 and first <= 239 then
                continuation_count = 2
                if first == 224 then minimum = 160 end

                if first == 237 then maximum = 159 end
            elseif first >= 240 and first <= 244 then
                continuation_count = 3
                if first == 240 then minimum = 144 end

                if first == 244 then maximum = 143 end
            else
                return false
            end

            if index + continuation_count > #value then return false end

            local second = string.byte(value, index + 1)
            if second < minimum or second > maximum then return false end

            for offset = 2, continuation_count do
                local continuation = string.byte(value, index + offset)
                if continuation < 128 or continuation > 191 then return false end
            end

            index = index + continuation_count + 1
        end
    end

    return true
end

local function encode_query(value)
    local encoded = {}
    for index = 1, #value do
        local byte = string.byte(value, index)
        if (byte >= 48 and byte <= 57)
            or (byte >= 65 and byte <= 90)
            or (byte >= 97 and byte <= 122)
            or byte == 45 or byte == 46 or byte == 95 or byte == 126 then
            encoded[#encoded + 1] = string.char(byte)
        else
            encoded[#encoded + 1] = string.format('%%%02X', byte)
        end
    end

    return table.concat(encoded)
end

local function open_home()
    return bahamut.open_url(wiki_origin .. '/wiki/Main_Page')
end

local function open_search(query)
    if #query == 0 or #query > maximum_query_bytes or not valid_utf8(query) then
        return false
    end

    local url = wiki_origin .. '/w/index.php?title=Special%3ASearch&search='
        .. encode_query(query)
    return bahamut.open_url(url)
end

function command(name)
    if name == '/wiki help' then
        bahamut.chat_print('/wiki <query> - search the Bahamut wiki')
        return true
    end

    if name == '/wiki' then
        if open_home() then
            bahamut.chat_print('Opening Bahamut wiki')
        else
            bahamut.chat_print('Unable to open Bahamut wiki')
        end

        return true
    end

    local query = string.sub(name, 7)
    if string.sub(name, 1, 6) == '/wiki ' and open_search(query) then
        bahamut.chat_print('Opening Bahamut wiki search')
        return true
    end

    if string.sub(name, 1, 6) == '/wiki ' then
        bahamut.chat_print('Wiki search query was rejected')
        return true
    end

    return false
end
