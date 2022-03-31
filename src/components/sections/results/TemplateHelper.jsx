import { TextField, Typography } from '@mui/material';
import React from 'react';
import { Trans, useTranslation } from 'react-i18next';
import { mapValues, parseDistribution } from '../../../utils/usefulFunctions';

const initial = {
  Power: 0,
  Burning: 0,
  Bleeding: 0,
  Poison: 0,
  Torment: 0,
  Confusion: 0,
};

const roundTwo = (num) => Math.round(num * 100) / 100;
const roundZero = (num) => Math.round(num);

const indent = (str, amount) => str.replace(/^/gm, ' '.repeat(amount));

// replaces "Poison" with "Poisoned" (keeping the same key order)
const fixPoison = (input) =>
  Object.fromEntries(
    Object.entries(input).map(([key, value]) => {
      const fixedKey = key === 'Poison' ? 'Poisoned' : key;
      return [fixedKey, value];
    }),
  );

const TemplateHelper = ({ character }) => {
  const { t } = useTranslation();

  const [input, setInput] = React.useState(initial);

  const [url, setUrl] = React.useState('');
  const [urlResult, setUrlResult] = React.useState('');

  React.useEffect(() => {
    async function fetchData() {
      setUrlResult('');
      if (url) {
        try {
          const permalink = url.split('/').slice(-1);
          if (!permalink) return;
          console.log('loading data from dps.report...');
          const response = await fetch(`https://dps.report/getJson?permalink=${permalink}`);
          const data = await response.json();
          console.log('got data from dps.report: ', data);

          if (data.error || !Array.isArray(data?.players)) {
            setUrlResult(JSON.stringify(data, null, 2));
            return;
          }

          const playerData = data.players.find((player) => player.name === data.recordedBy);

          if (!playerData) {
            setUrlResult('no player data!');
            return;
          }

          const duration = (data.phases?.[0]?.end - data.phases?.[0]?.start) / 1000;

          let sum = 0;
          const powerDPS = playerData.dpsTargets?.[0]?.[0]?.powerDps;
          sum += powerDPS;

          const conditionIds = {
            Burning: 737,
            Bleeding: 736,
            Poison: 723,
            Torment: 19426,
            Confusion: 861,
          };

          const conditionData = mapValues(conditionIds, (id) => {
            const damage = playerData.targetDamageDist?.[0]?.[0]?.find(
              (entry) => entry?.id === id,
            )?.totalDamage;
            const dps = roundTwo((damage ?? 0) / duration);
            sum += dps;
            return dps;
          });

          const totalDPS = playerData.dpsTargets?.[0]?.[0]?.dps;

          const hits = playerData.statsTargets?.[0]?.[0]?.critableDirectDamageCount;
          const crits = playerData.statsTargets?.[0]?.[0]?.criticalRate;

          const hitsPerSecond = hits / duration;
          const critPercent = (crits / hits) * 100;

          const minions = playerData.minions ?? [];

          const minionCounts = {
            'Clone': { names: new Set(), minionHits: 0, minionCrits: 0 },
            'Phantasm': { names: new Set(), minionHits: 0, minionCrits: 0 },
            'Minion': { names: new Set(), minionHits: 0, minionCrits: 0 },
          };

          for (const { name, targetDamageDist } of minions) {
            console.log(name);
            let type = 'Minion';
            if (name === 'Clone') type = 'Clone';
            if (name?.startsWith('Illusionary')) type = 'Phantasm';

            minionCounts[type].names.add(name);

            for (const skill of targetDamageDist?.[0]?.[0] ?? []) {
              const { indirectDamage, connectedHits: minionHits, crit: minionCrits } = skill;
              if (indirectDamage) continue;
              console.log(minionCrits, minionHits);
              minionCounts[type].minionHits += minionHits ?? 0;
              minionCounts[type].minionCrits += minionCrits ?? 0;
            }
          }

          const minionData = Object.entries(minionCounts)
            .filter(([_type, { minionHits }]) => minionHits)
            .flatMap(([type, { names, minionHits, minionCrits }]) => {
              const namesString = [...names].join(', ');

              const minionHitsPerSecond = minionHits / duration;
              const minionCritPercent = (minionCrits / minionHits) * 100;

              return [
                [
                  `${type} hits/sec (${minionCrits}/${minionHits}: ${minionCritPercent.toFixed(
                    2,
                  )}% crit)`,
                  minionHitsPerSecond,
                ],
                `            - ${namesString}\n`,
              ];
            });

          const result = [
            ['Duration (sec)', duration],
            '\n',
            ['Power DPS (including minions)', powerDPS],
            ...Object.entries(conditionData).map(([key, value]) => [`${key} DPS`, value]),
            '\n',
            ['Sum', sum],
            ['Total dps (log)', totalDPS],
            '\n',
            [
              `Player crittable hits per second (${crits}/${hits}: ${critPercent.toFixed(
                2,
              )}% crit)`,
              hitsPerSecond,
            ],
            '\n',
            ...minionData,
          ];

          const resultAreaText = result
            .map((item) => {
              if (typeof item === 'string') return item;
              const [key, value] = item;
              return `${String(value.toFixed(2)).padStart(9)}: ${key}`;
            })
            .join('\n');

          setInput({ Power: powerDPS, ...conditionData });
          setUrlResult(resultAreaText);
        } catch (e) {
          console.error(e);
          setUrlResult(String(e));
        }
      }
    }
    fetchData();
  }, [url]);

  const data = Object.entries(input).map(([key, inputText]) => {
    const { value, error } = parseDistribution(inputText);
    return { key, inputText, value, error };
  });

  const { cachedFormState } = character.settings;
  const { coefficientHelper } = character.results;

  // reverse engineer coefficient needed to reach target damage
  // DPS = slope * coefficient + intercept
  // coefficient = (DPS - intercept) / slope
  const calculateRequiredCoefficient = (key, targetDPS = 0) => {
    const { slope, intercept } = coefficientHelper[key];
    if (targetDPS === intercept) return 0;
    return (targetDPS - intercept) / slope;
  };

  let values2 = Object.fromEntries(
    data.map(({ key, value }) => [key, calculateRequiredCoefficient(key, value)]),
  );

  // round
  Object.keys(values2).forEach((key) => {
    values2[key] = key === 'Power' ? roundZero(values2[key]) : roundTwo(values2[key]);
  });

  values2 = fixPoison(values2);

  const distribution = { values2 };

  const formattedDistribution = JSON.stringify(distribution, null, 2)
    .replaceAll('\n    ', ' ')
    .replaceAll('\n  }', ' }');

  return (
    <>
      <Typography variant="h6">
        <Trans>Distribution Template</Trans>
      </Typography>
      <Typography variant="caption">
        <Trans>input the DPS values from a golem log here:</Trans>
      </Typography>

      <table>
        <tbody>
          <tr>
            {data.map(({ key }) => (
              <td key={key}>
                {
                  // i18next-extract-mark-context-next-line ["Power","Burning","Bleeding","Poison","Torment", "Confusion"]
                  t('DPSType', { context: key })
                }
              </td>
            ))}
          </tr>
          <tr>
            {data.map(({ key, inputText, error }) => {
              return (
                <td key={key}>
                  <TextField
                    variant="standard"
                    error={error}
                    value={inputText}
                    onChange={(e) => {
                      const newInput = { ...input };
                      newInput[key] = e.target.value;
                      setInput(newInput);
                    }}
                  />
                </td>
              );
            })}
          </tr>
        </tbody>
      </table>

      <br />

      <Typography variant="caption">
        <Trans>or, enter a dps.report URL to attempt to to fetch the data automatically:</Trans>
      </Typography>
      <br />
      <TextField
        fullWidth
        variant="standard"
        onChange={(e) => {
          setUrl(e.target.value);
        }}
      />
      <pre style={{ margin: '1rem' }}>{urlResult}</pre>

      <br />

      <Typography variant="caption">
        <Trans>result:</Trans>
      </Typography>

      <table>
        <tbody>
          <tr>
            {Object.keys(values2).map((key) => {
              const type = key === 'Power' ? 'Power Coefficient' : `Avg. ${key} Stacks`;
              return <td key={type}>{type}</td>;
            })}
          </tr>
          <tr>
            {Object.values(values2).map((value, index) => (
              // eslint-disable-next-line react/no-array-index-key
              <td key={index}>
                <TextField disabled value={value} variant="standard" />
              </td>
            ))}
          </tr>
        </tbody>
      </table>

      <pre style={{ userSelect: 'all', overflowY: 'auto', maxHeight: '250px', margin: '1rem' }}>
        {indent(formattedDistribution, 6)}
      </pre>

      <Typography variant="h6">
        <Trans>Trait Template</Trans>
      </Typography>

      <pre style={{ userSelect: 'all', overflowY: 'auto', maxHeight: '250px', margin: '1rem' }}>
        {indent(JSON.stringify(cachedFormState?.traits, null, 2) || '', 6)}
      </pre>

      <Typography variant="h6">
        <Trans>Skills Template</Trans>
      </Typography>

      <pre style={{ userSelect: 'all', overflowY: 'auto', maxHeight: '250px', margin: '1rem' }}>
        {indent(JSON.stringify(cachedFormState?.skills, null, 2) || '', 6)}
      </pre>

      <Typography variant="h6">
        <Trans>Extras Template</Trans>
      </Typography>

      <pre style={{ userSelect: 'all', overflowY: 'auto', maxHeight: '250px', margin: '1rem' }}>
        {indent(JSON.stringify(cachedFormState?.extras, null, 2) || '', 6)}
      </pre>
    </>
  );
};

export default React.memo(TemplateHelper);
