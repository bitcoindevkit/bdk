# issue with checksums

```
docker compose up -d
docker compose exec bitcoind bitcoin-cli createwallet default
docker compose exec bitcoind bitcoin-cli -regtest -rpcwallet=default importdescriptors '[{"desc":"pkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/44h/1h/0h/0/*)#lasegmsf","timestamp":"now"}]'
docker compose exec bitcoind bitcoin-cli -regtest getdescriptorinfo "pkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/44h/1h/0h/0/*)"
{
  "descriptor": "pkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/44'/1'/0'/0/*)#lasegmfs",
  "checksum": "nslaf9cz",
  "isrange": true,
  "issolvable": true,
  "hasprivatekeys": false
}
```
