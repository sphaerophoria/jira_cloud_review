# Jira summary tool

Sometimes you want to look at what you did over a period of time. Maybe it's the end of sprint, and you have to tell your coworkers what you did, but you can't even remember what you worked on yesterday. You know you've left comments on jira, but they aren't easy to find. JQL lets you search for updated tickets, but not tickets that you commented on in a certain date range. There are addons you can download for jira that add this functionality, but you aren't a jira admin.

In my case, I happen to have access to the jira rest api. I can use that to get the info I want.

This tool retrieves all comments that I've made after a provided date, and wraps them up into a nice little html file that I can read to refresh my memory
