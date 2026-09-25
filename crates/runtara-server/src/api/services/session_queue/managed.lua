-- Every key is scoped to one tenant/session hash slot. Payload JSON remains a
-- string: Lua cjson must never round an integer inside a user's response.
local expected = {'list', 'hash', 'hash', 'zset', 'hash'}
for i, kind in ipairs(expected) do
    local actual = redis.call('TYPE', KEYS[i]).ok
    if actual ~= 'none' and actual ~= kind then return {'corrupt'} end
end
local tenant, session = ARGV[#ARGV-1], ARGV[#ARGV]
local owner = redis.call('HMGET', KEYS[5], 'tenant_id', 'session_id')
if owner[1] or owner[2] then
    if owner[1] ~= tenant or owner[2] ~= session then return {'corrupt'} end
else
    if redis.call('EXISTS', KEYS[5]) == 1 then return {'corrupt'} end
    for i=1,4 do
        if redis.call('EXISTS', KEYS[i]) == 1 then return {'corrupt'} end
    end
end
local clock = redis.call('TIME')
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
local op = ARGV[1]
local function load(id)
    local raw = redis.call('HGET', KEYS[2], id)
    if not raw then return nil end
    local ok, e = pcall(cjson.decode, raw)
    if not ok or type(e) ~= 'table' or e.message_id ~= id or
       type(e.operation_id) ~= 'string' or type(e.payload_json) ~= 'string' or
       type(e.state) ~= 'string' or type(e.attempts) ~= 'number' or
       type(e.enqueued_at_ms) ~= 'number' then return nil end
    if e.state ~= 'queued' and e.state ~= 'leased' and e.state ~= 'retry' and
       e.state ~= 'blocked' and e.state ~= 'accepted' and e.state ~= 'failed' then return nil end
    if e.state == 'leased' and (type(e.lease_token) ~= 'string' or type(e.lease_deadline_ms) ~= 'number') then return nil end
    if e.state == 'retry' and type(e.retry_at_ms) ~= 'number' then return nil end
    if e.target ~= nil and e.target ~= cjson.null and
       (type(e.target) ~= 'table' or type(e.target.instance_id) ~= 'string' or type(e.target.request_id) ~= 'string') then return nil end
    if (e.state == 'blocked' or e.state == 'retry' or e.state == 'failed') and type(e.reason) ~= 'string' then return nil end
    if (e.state == 'accepted' or e.state == 'failed') and type(e.completed_at_ms) ~= 'number' then return nil end
    if e.state == 'accepted' and (type(e.receipt_id) ~= 'string' or type(e.target) ~= 'table') then return nil end
    if redis.call('HGET', KEYS[3], e.operation_id) ~= id then return nil end
    local valid_payload = pcall(cjson.decode, e.payload_json)
    if not valid_payload then return nil end
    return e
end
local function save(e)
    local raw = cjson.encode(e)
    redis.call('HSET', KEYS[2], e.message_id, raw)
    return {'ok', raw}
end
local function unlease(e)
    e.lease_token = nil
    e.lease_deadline_ms = nil
end
local function same_target(a,b)
    return a.instance_id == b.instance_id and a.request_id == b.request_id
end
local function finish(e)
    unlease(e)
    e.completed_at_ms = now
    -- Retention starts only after acceptance or explicit failure.
    local result = save(e)
    redis.call('LPOP', KEYS[1])
    redis.call('ZADD', KEYS[4], now + tonumber(ARGV[6]), e.message_id)
    return result
end

local function route()
    local raw = redis.call('HGET', KEYS[5], 'route_json')
    if not raw then return nil end
    local ok, value = pcall(cjson.decode, raw)
    if not ok or type(value) ~= 'table' or type(value.instance_id) ~= 'string' or value.instance_id == '' or
       type(value.workflow_id) ~= 'string' or value.workflow_id == '' then return nil end
    return value
end
if op == 'has_unresolved' then
    return {'ok', tostring(redis.call('LLEN', KEYS[1]))}
elseif op == 'configure_route' then
    local current = redis.call('HGET', KEYS[5], 'route_json')
    if current and current ~= ARGV[2] and redis.call('LLEN', KEYS[1]) > 0 then return {'conflict'} end
    local next_route = cjson.decode(ARGV[2])
    if current then
        local old = route()
        if not old then return {'corrupt'} end
        if old.workflow_id ~= next_route.workflow_id then return {'conflict'} end
    end
    redis.call('HSET', KEYS[5], 'tenant_id', tenant, 'session_id', session, 'route_json', ARGV[2])
    redis.call('PERSIST', KEYS[5])
    return {'ok', ARGV[2]}
elseif op == 'session_route' then
    local raw = redis.call('HGET', KEYS[5], 'route_json')
    if not raw then return {'not_found'} end
    if not route() then return {'corrupt'} end
    return {'ok', raw}
end

if op == 'enqueue' then
    local id, operation, payload = ARGV[2], ARGV[3], ARGV[4]
    local selected
    if ARGV[5] ~= '' then
        local ok, target = pcall(cjson.decode, ARGV[5])
        if not ok or type(target) ~= 'table' or type(target.instance_id) ~= 'string' or target.instance_id == '' or
           type(target.request_id) ~= 'string' or target.request_id == '' then return {'invalid'} end
        selected = target
    end
    local raw = redis.call('HGET', KEYS[2], id)
    if raw then
        local e = load(id)
        if not e then return {'corrupt'} end
        if e.operation_id ~= operation or e.payload_json ~= payload then return {'conflict'} end
        if selected and (e.target == nil or e.target == cjson.null or not same_target(e.target, selected)) then return {'conflict'} end
        return {'ok', raw}
    end
    local existing = redis.call('HGET', KEYS[3], operation)
    if existing then return {'conflict'} end
    local e = {message_id=id, operation_id=operation, payload_json=payload,
               state='queued', enqueued_at_ms=now, attempts=0, target=selected}
    local result = save(e)
    redis.call('HSET', KEYS[5], 'tenant_id', tenant, 'session_id', session)
    redis.call('PERSIST', KEYS[5])
    redis.call('HSET', KEYS[3], operation, id)
    redis.call('RPUSH', KEYS[1], id)
    return result
elseif op == 'scan_envelopes' then
    local page = redis.call('HSCAN', KEYS[2], ARGV[2], 'COUNT', ARGV[3])
    local raw = {}
    for i=1,#page[2],2 do
        if not load(page[2][i]) then return {'corrupt'} end
        table.insert(raw, page[2][i+1])
    end
    return {'ok', '{"cursor":' .. page[1] .. ',"envelopes":[' .. table.concat(raw, ',') .. ']}'}
elseif op == 'get' then
    local raw = redis.call('HGET', KEYS[2], ARGV[2])
    if not raw then return {'not_found'} end
    if not load(ARGV[2]) then return {'corrupt'} end
    return {'ok', raw}
elseif op == 'prune' then
    local ids = redis.call('ZRANGEBYSCORE', KEYS[4], '-inf', now, 'LIMIT', 0, tonumber(ARGV[2]))
    local envelopes = {}
    -- Validate the entire bounded batch before making any writes.
    for _, id in ipairs(ids) do
        local e = load(id)
        if not e or (e.state ~= 'accepted' and e.state ~= 'failed') then return {'corrupt'} end
        table.insert(envelopes, e)
    end
    for _, e in ipairs(envelopes) do
        redis.call('HDEL', KEYS[2], e.message_id)
        redis.call('HDEL', KEYS[3], e.operation_id)
        redis.call('ZREM', KEYS[4], e.message_id)
    end
    if redis.call('HLEN', KEYS[2]) == 0 and redis.call('LLEN', KEYS[1]) == 0 and not redis.call('HGET', KEYS[5], 'route_json') then redis.call('DEL', KEYS[5]) end
    return {'ok', tostring(#ids)}
end

local head = redis.call('LINDEX', KEYS[1], 0)
if not head then return {'empty'} end
local e = load(head)
if not e or e.state == 'accepted' or e.state == 'failed' then return {'corrupt'} end
if op == 'claim' then
    if e.state == 'blocked' then return {'blocked', cjson.encode(e)} end
    if e.state == 'leased' and e.lease_deadline_ms > now then return {'busy'} end
    if e.state == 'retry' and e.retry_at_ms > now then return {'deferred', cjson.encode(e)} end
    e.state = 'leased'
    e.lease_token = ARGV[2]
    e.lease_deadline_ms = now + tonumber(ARGV[3])
    e.attempts = e.attempts + 1
    e.retry_at_ms = nil
    return save(e)
end
if head ~= ARGV[2] then return {'lease_lost'} end

if op == 'resolve' or op == 'fail_blocked' then
    if e.state ~= 'blocked' then return {'conflict'} end
    if op == 'fail_blocked' then
        e.state = 'failed'
        e.reason = 'explicit_failure'
        return finish(e)
    end
    local target = cjson.decode(ARGV[4])
    if e.target ~= nil and e.target ~= cjson.null then
        if not same_target(e.target, target) then return {'conflict'} end
    else e.target = target end
    e.state = 'queued'
    e.reason = nil
    return save(e)
end

if e.state ~= 'leased' or e.lease_token ~= ARGV[3] or e.lease_deadline_ms <= now then
    return {'lease_lost'}
end
if op == 'bind' then
    local target = cjson.decode(ARGV[4])
    if e.target ~= nil and e.target ~= cjson.null then
        if not same_target(e.target, target) then return {'conflict'} end
    else e.target = target end
elseif op == 'renew' then
    e.lease_deadline_ms = now + tonumber(ARGV[4])
elseif op == 'ack' then
    local receipt = cjson.decode(ARGV[4])
    if not e.target or e.target == cjson.null or receipt.request_id ~= e.target.request_id or receipt.operation_id ~= e.operation_id then
        return {'conflict'}
    end
    e.receipt_id = receipt.receipt_id
    e.state = 'accepted'
    e.reason = nil
    return finish(e)
elseif op == 'retry' then
    e.state = 'retry'
    e.reason = ARGV[4]
    e.retry_at_ms = now + tonumber(ARGV[5])
    unlease(e)
elseif op == 'block' then
    e.state = 'blocked'
    e.reason = ARGV[4]
    unlease(e)
else return {'invalid'} end
return save(e)
