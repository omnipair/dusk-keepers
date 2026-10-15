export function flattenAccounts(accounts) {
  return accounts.flatMap((account) =>
    account.accounts ? flattenAccounts(account.accounts) : [account],
  );
}
