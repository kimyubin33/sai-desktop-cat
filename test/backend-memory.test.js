const test = require('node:test');
const { defineContract } = require('./backend-contract');
const { MemoryBackend, emptyHub } = require('../src/core/team/memory-backend');

const hub = emptyHub();
defineContract(test, async (n) => Array.from({ length: n }, () => new MemoryBackend({ hub })));
