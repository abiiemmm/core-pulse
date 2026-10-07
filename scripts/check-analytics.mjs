import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
const source = ts.transpileModule(fs.readFileSync('src/analytics-data.ts','utf8'),{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText;
const { sensorStatistics, chartSeries, analyticsCsv } = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
const report = {
  from:'2026-10-04T10:00:00Z',to:'2026-10-04T10:00:20Z',bucket_seconds:5,
  points:[
    {sensor_key:'cpu_usage',recorded_at:'2026-10-04T10:00:00Z',average:50,minimum:20,maximum:80,sample_count:9},
    {sensor_key:'cpu_usage',recorded_at:'2026-10-04T10:00:10Z',average:0,minimum:0,maximum:0,sample_count:1},
  ],
};
assert.deepEqual(sensorStatistics(report.points,'cpu_usage'),{count:10,average:45,minimum:0,maximum:80});
assert.deepEqual(sensorStatistics(report.points,'gpu_usage'),{count:0,average:null,minimum:null,maximum:null});
assert.deepEqual(chartSeries(report,['cpu_usage','gpu_usage']).values,[[50,null,0,null,null],[null,null,null,null,null]]);
const csv = analyticsCsv(report);
assert.equal(csv.split('\r\n').length,3);
assert(csv.includes('"cpu_usage","%","0","0","0","1"'));
const messages=JSON.parse(fs.readFileSync('src/locales.json','utf8'));
for(const [key,translations] of Object.entries(messages)) {
  assert.equal(translations.length,2,key);
  for(const translation of translations) assert.deepEqual((translation.match(/\{\w+\}/g)??[]).sort(),(key.match(/\{\w+\}/g)??[]).sort(),key);
}
for (const file of ['src/Processes.tsx', 'src/SensorPanel.tsx', 'src/Gaming.tsx', 'src/DesktopSettings.tsx']) for (const [, key] of fs.readFileSync(file,'utf8').matchAll(/\bt\('([^']+)'/g)) {
  assert(messages[key], `Process translation missing: ${key}`);
  for (const translation of messages[key]) assert(!translation.includes('?') && !translation.includes('\uFFFD'), `Damaged translation: ${key}`);
}
console.log('Analytics weighting, missing data, zero values, CSV, and translation placeholders passed.');
