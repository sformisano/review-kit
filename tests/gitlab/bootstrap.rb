# Only fixture setup uses this administrator token. Adapter tests create ordinary users.
root = User.find_by!(username: 'root')
root.personal_access_tokens.where(name: 'review-kit-local-fixtures').destroy_all
token = root.personal_access_tokens.create!(
  name: 'review-kit-local-fixtures', scopes: ['api'], expires_at: Date.today + 7
)
Gitlab::CurrentSettings.update!(signup_enabled: false)
puts token.token
